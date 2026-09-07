# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0

from __future__ import annotations

import re
import shutil
from pathlib import Path

from .common import (
    APKSigning,
    GPGSigning,
    TaskError,
    controlled_environment,
    required_env,
)
from .storage import S3Storage

TASK = "publish"
KEY_VERSION = re.compile(r"^[0-9]{4}$")
RSA_PUBLIC_KEY = re.compile(r"^keys/packages\.[0-9]{4}\.rsa\.pub$")


class Repository:
    def __init__(self, service, channel, input_directory, formats, work):
        self.service = service
        self.channel = channel
        self.formats = formats
        self.work = work
        self.public_url = required_env("S3_PUBLIC_URL", TASK).rstrip("/")
        self.key_version = required_env("PACKAGE_KEY_VERSION", TASK)
        if not KEY_VERSION.fullmatch(self.key_version):
            raise TaskError(f"{TASK}: invalid PACKAGE_KEY_VERSION: {self.key_version}")
        self.storage = S3Storage(TASK, service)
        self.signing_environment = controlled_environment()
        self.gpg = None
        self.gpg_public_key = None
        self.apk_signing = None
        self.apk_public_keys = []
        self.packages = {
            package_format: sorted(input_directory.glob(f"*.{package_format}"))
            for package_format in formats
        }
        for package_format, packages in self.packages.items():
            if not packages:
                raise TaskError(
                    f"{TASK}: no .{package_format} packages found in {input_directory}"
                )

    def check_public_key(self, source: Path, key: str) -> None:
        existing = self.work / f"existing-{source.name}"
        if not self.storage.download(key, existing):
            raise TaskError(f"{TASK}: organization key {key} is not provisioned")
        if source.read_bytes() != existing.read_bytes():
            raise TaskError(f"{TASK}: signing key does not match {key}")

    @staticmethod
    def add_package(source: Path, directory: Path) -> None:
        destination = directory / source.name
        if destination.exists() and source.read_bytes() != destination.read_bytes():
            raise TaskError(
                f"{TASK}: immutable package filename has different content: {source.name}"
            )
        if not destination.exists():
            shutil.copy2(source, destination)

    async def setup_openpgp(self) -> None:
        self.gpg = await GPGSigning.create(
            TASK, self.work, environment=self.signing_environment
        )
        current = self.work / "current-packages.gpg"
        await self.gpg.export_public_key(current)
        self.check_public_key(current, f"keys/packages.{self.key_version}.gpg")

        bundle = self.work / "packages.gpg"
        if not self.storage.download("packages.gpg", bundle):
            raise TaskError(f"{TASK}: organization key packages.gpg is not provisioned")
        await self.gpg.verify_public_bundle(bundle)
        self.gpg_public_key = bundle

    async def setup_rsa(self) -> None:
        key_name = f"packages.{self.key_version}"
        self.apk_signing = APKSigning(
            TASK, self.work, key_name, environment=self.signing_environment
        )
        current = self.work / self.apk_signing.public_key_name
        await self.apk_signing.export_public_key(current)
        self.check_public_key(
            current,
            f"keys/{self.apk_signing.public_key_name}",
        )

        key_directory = self.work / "rsa-public-keys"
        key_directory.mkdir()
        for key in sorted(self.storage.objects("keys/")):
            if RSA_PUBLIC_KEY.fullmatch(key):
                destination = key_directory / Path(key).name
                if destination.exists():
                    raise TaskError(f"{TASK}: duplicate RSA public key name: {key}")
                self.storage.download(key, destination)
                self.apk_public_keys.append(destination)
        if current.name not in {key.name for key in self.apk_public_keys}:
            raise TaskError(
                f"{TASK}: current RSA public key is absent from key archive"
            )

    async def setup_signing(self) -> None:
        if {"deb", "rpm"} & set(self.formats):
            await self.setup_openpgp()
        if "apk" in self.formats:
            await self.setup_rsa()

    async def publish(self) -> None:
        from .apk import publish as publish_apk
        from .apt import publish as publish_apt
        from .rpm import publish as publish_rpm

        publishers = {"deb": publish_apt, "rpm": publish_rpm, "apk": publish_apk}
        with self.storage.lock(self.channel):
            await self.setup_signing()
            for package_format in self.formats:
                await publishers[package_format](self)
