# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

import hashlib
import io
import os
import shutil
from pathlib import Path

from botocore.exceptions import ClientError


def error(code: str, operation: str) -> ClientError:
    return ClientError({"Error": {"Code": code, "Message": code}}, operation)


class Paginator:
    def __init__(self, client):
        self.client = client

    def paginate(self, *, Bucket: str, Prefix: str):
        root = self.client.root / Bucket
        contents = []
        if root.is_dir():
            for path in root.rglob("*"):
                if path.is_file():
                    key = path.relative_to(root).as_posix()
                    if key.startswith(Prefix):
                        contents.append({"Key": key, "Size": path.stat().st_size})
        yield {"Contents": contents}


class Client:
    def __init__(self):
        self.root = Path(os.environ["PUBLISH_REMOTE"])
        self.metadata: dict[tuple[str, str], dict[str, str]] = {}
        self.log = os.environ.get("PUBLISH_TEST_LOG")

    def record(self, operation: str, key: str) -> None:
        if self.log:
            with Path(self.log).open("a") as stream:
                stream.write(f"s3 {operation} {key}\n")

    def path(self, bucket: str, key: str) -> Path:
        path = self.root / bucket / key
        path.resolve().relative_to(self.root.resolve())
        return path

    def get_paginator(self, name: str):
        assert name == "list_objects_v2"
        return Paginator(self)

    def download_file(self, bucket: str, key: str, destination: str) -> None:
        source = self.path(bucket, key)
        if not source.is_file():
            raise error("NoSuchKey", "DownloadFile")
        Path(destination).parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        self.record("download", key)

    def upload_file(self, source: str, bucket: str, key: str) -> None:
        destination = self.path(bucket, key)
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        self.record("upload", key)

    def put_object(
        self,
        *,
        Bucket,
        Key,
        Body,
        Metadata=None,
        IfNoneMatch=None,
        IfMatch=None,
    ):
        destination = self.path(Bucket, Key)
        if IfNoneMatch == "*" and destination.exists():
            raise error("PreconditionFailed", "PutObject")
        if IfMatch and self.etag(destination) != IfMatch:
            raise error("PreconditionFailed", "PutObject")
        destination.parent.mkdir(parents=True, exist_ok=True)
        data = Body.read() if hasattr(Body, "read") else Body
        destination.write_bytes(data)
        self.metadata[(Bucket, Key)] = Metadata or {}
        self.record("immutable", Key)
        return {"ETag": self.etag(destination)}

    def head_object(self, *, Bucket, Key):
        path = self.path(Bucket, Key)
        if not path.exists():
            raise error("NoSuchKey", "HeadObject")
        return {"Metadata": self.metadata.get((Bucket, Key), {})}

    @staticmethod
    def etag(path: Path) -> str | None:
        if not path.exists():
            return None
        return f'"{hashlib.md5(path.read_bytes(), usedforsecurity=False).hexdigest()}"'

    def get_object(self, *, Bucket, Key):
        path = self.path(Bucket, Key)
        return {"Body": io.BytesIO(path.read_bytes()), "ETag": self.etag(path)}

    def delete_object(self, *, Bucket, Key, IfMatch):
        path = self.path(Bucket, Key)
        if self.etag(path) != IfMatch:
            raise error("PreconditionFailed", "DeleteObject")
        path.unlink()
        self.record("delete", Key)
        return {}

    def delete_objects(self, *, Bucket, Delete):
        for item in Delete["Objects"]:
            self.path(Bucket, item["Key"]).unlink(missing_ok=True)
            self.record("delete", item["Key"])
        return {}


def client(name: str, **kwargs):
    assert name == "s3"
    assert kwargs["endpoint_url"]
    assert kwargs["aws_access_key_id"]
    assert kwargs["aws_secret_access_key"]
    return Client()
