# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

import fnmatch
import hashlib
import json
import time
import uuid
from contextlib import contextmanager
from pathlib import Path

import boto3
from botocore.exceptions import ClientError

from .common import TaskError, required_env


class S3Storage:
    def __init__(self, task: str, service: str):
        self.task = task
        self.bucket = required_env("S3_BUCKET", task)
        self.service_root = service
        self.client = boto3.client(
            "s3",
            endpoint_url=required_env("S3_ENDPOINT", task),
            aws_access_key_id=required_env("S3_ACCESS_KEY_ID", task),
            aws_secret_access_key=required_env("S3_SECRET_ACCESS_KEY", task),
            region_name="auto",
        )

    def service_key(self, *parts: str) -> str:
        return "/".join((self.service_root, *parts))

    def download(self, key: str, destination: Path) -> bool:
        destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            self.client.download_file(self.bucket, key, str(destination))
        except ClientError as error:
            if error.response.get("Error", {}).get("Code") in {
                "404",
                "NoSuchKey",
                "NotFound",
            }:
                return False
            raise
        return True

    def objects(self, prefix: str) -> set[str]:
        pages = self.client.get_paginator("list_objects_v2").paginate(
            Bucket=self.bucket, Prefix=prefix
        )
        return {item["Key"] for page in pages for item in page.get("Contents", [])}

    def download_prefix(self, prefix: str, destination: Path, pattern: str) -> None:
        destination.mkdir(parents=True, exist_ok=True)
        prefix = prefix.rstrip("/") + "/"
        for key in self.objects(prefix):
            relative = key.removeprefix(prefix).lstrip("/")
            if relative and "/" not in relative and fnmatch.fnmatch(relative, pattern):
                self.client.download_file(self.bucket, key, str(destination / relative))

    @staticmethod
    def digest(path: Path) -> str:
        with path.open("rb") as stream:
            return hashlib.file_digest(stream, "sha256").hexdigest()

    @staticmethod
    def conflict(error: ClientError) -> bool:
        return error.response.get("Error", {}).get("Code") in {
            "409",
            "412",
            "ConditionalRequestConflict",
            "PreconditionFailed",
        }

    def upload_immutable(self, source: Path, key: str) -> None:
        digest = self.digest(source)
        try:
            with source.open("rb") as stream:
                self.client.put_object(
                    Bucket=self.bucket,
                    Key=key,
                    Body=stream,
                    Metadata={"sha256": digest},
                    IfNoneMatch="*",
                )
            return
        except ClientError as error:
            if not self.conflict(error):
                raise

        existing = self.client.head_object(Bucket=self.bucket, Key=key)
        existing_digest = existing.get("Metadata", {}).get("sha256")
        if not existing_digest:
            body = self.client.get_object(Bucket=self.bucket, Key=key)["Body"]
            existing_digest = hashlib.sha256(body.read()).hexdigest()
        if existing_digest != digest:
            raise TaskError(
                f"{self.task}: immutable object has different content: {key}"
            )

    def upload_payloads(self, source: Path, prefix: str, pattern: str) -> None:
        for path in sorted(source.glob(pattern)):
            self.upload_immutable(path, f"{prefix.rstrip('/')}/{path.name}")

    def upload(self, source: Path, key: str) -> None:
        self.client.upload_file(str(source), self.bucket, key)

    @contextmanager
    def lock(self, name: str, lifetime: int = 3600):
        key = self.service_key("_locks", name)
        body = json.dumps(
            {"expires": int(time.time()) + lifetime, "id": uuid.uuid4().hex}
        )
        try:
            result = self.client.put_object(
                Bucket=self.bucket, Key=key, Body=body.encode(), IfNoneMatch="*"
            )
        except ClientError as error:
            if not self.conflict(error):
                raise
            current = self.client.get_object(Bucket=self.bucket, Key=key)
            state = json.loads(current["Body"].read())
            if state["expires"] > time.time():
                raise TaskError(f"{self.task}: publication already in progress: {name}")
            result = self.client.put_object(
                Bucket=self.bucket,
                Key=key,
                Body=body.encode(),
                IfMatch=current["ETag"],
            )
        try:
            yield
        finally:
            self.client.delete_object(
                Bucket=self.bucket, Key=key, IfMatch=result["ETag"]
            )

    def replace_prefix(self, source: Path, prefix: str) -> None:
        prefix = prefix.rstrip("/") + "/"
        wanted: set[str] = set()
        for path in sorted(item for item in source.rglob("*") if item.is_file()):
            key = prefix + path.relative_to(source).as_posix()
            wanted.add(key)
            self.client.upload_file(str(path), self.bucket, key)
        stale = sorted(set(self.objects(prefix)) - wanted)
        for offset in range(0, len(stale), 1000):
            self.client.delete_objects(
                Bucket=self.bucket,
                Delete={
                    "Objects": [{"Key": key} for key in stale[offset : offset + 1000]],
                    "Quiet": True,
                },
            )
