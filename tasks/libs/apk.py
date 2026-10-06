# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

import asyncio
import gzip
import hashlib
import io
import os
import platform
import shutil
import tarfile
import urllib.request
from pathlib import Path

from shellous import sh

from .common import TaskError, capture, run

TASK = "publish"
APK_TOOLS_VERSION = "2.14.10"
APK_TOOLS_SHA256 = {
    "x86_64": "34bb1a96f0258982377a289392d4ea9f3f4b767a4bb5806b1b87179b79ad8a1c",
    "aarch64": "e471d35aa221d031abe9b6288aede12a8e9f1a398954e5a2e1d1bce1727b4ef4",
}


async def architecture(package: Path) -> str:
    metadata = await capture("tar", "-xOzf", package, ".PKGINFO").stderr(sh.DEVNULL)
    for line in metadata.splitlines():
        if line.startswith("arch = "):
            return line.removeprefix("arch = ")
    raise TaskError(f"{TASK}: cannot read APK architecture: {package}")


async def apk_tool(work: Path) -> Path:
    configured = os.environ.get("APK_TOOL")
    if configured:
        tool = Path(configured)
        if os.access(tool, os.X_OK):
            return tool
        raise TaskError(f"{TASK}: APK_TOOL is not executable: {tool}")
    for name in ("apk.static", "apk"):
        if command := shutil.which(name):
            return Path(command)

    machine = platform.machine()
    apk_arch = {
        "x86_64": "x86_64",
        "amd64": "x86_64",
        "aarch64": "aarch64",
        "arm64": "aarch64",
    }.get(machine)
    if apk_arch is None:
        raise TaskError(f"{TASK}: apk-tools is unavailable for {machine}")
    tool = work / "apk.static"
    url = (
        "https://gitlab.alpinelinux.org/api/v4/projects/5/packages/generic/"
        f"v{APK_TOOLS_VERSION}/{apk_arch}/apk.static"
    )
    await asyncio.to_thread(urllib.request.urlretrieve, url, tool)
    if hashlib.sha256(tool.read_bytes()).hexdigest() != APK_TOOLS_SHA256[apk_arch]:
        raise TaskError(f"{TASK}: apk-tools checksum mismatch")
    tool.chmod(0o755)
    return tool


async def sign_index(context, index: Path) -> None:
    name = f".SIGN.RSA256.{context.apk_signing.public_key_name}"
    signature = index.parent / name
    await run.set(env=context.apk_signing.environment, inherit_env=False)(
        "openssl",
        "dgst",
        "-sha256",
        "-sign",
        context.apk_signing.private_key_file,
        "-out",
        signature,
        index,
    )
    data = signature.read_bytes()
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        information = tarfile.TarInfo(name)
        information.size = len(data)
        information.mode = 0o644
        information.mtime = int(os.environ.get("SOURCE_DATE_EPOCH", "0"))
        archive.addfile(information, io.BytesIO(data))
    size = 512 + ((len(data) + 511) // 512) * 512
    index.write_bytes(
        gzip.compress(stream.getvalue()[:size], mtime=0) + index.read_bytes()
    )
    signature.unlink()


async def publish(context) -> None:
    tool = await apk_tool(context.work)
    packages = context.packages["apk"]
    package_architectures = dict(
        zip(
            packages,
            await asyncio.gather(*(architecture(package) for package in packages)),
            strict=True,
        )
    )
    architectures = sorted(set(package_architectures.values()))
    keys = context.work / "apk-keys"
    keys.mkdir()
    for public_key in context.apk_public_keys:
        shutil.copy2(public_key, keys / public_key.name)

    for apk_arch in architectures:
        root = context.work / "apk" / apk_arch
        root.mkdir(parents=True)
        remote = context.storage.service_key("apk", context.channel, apk_arch)
        context.storage.download_prefix(remote, root, "*.apk")
        for package in packages:
            if package_architectures[package] == apk_arch:
                context.add_package(package, root)
        for package in root.glob("*.apk"):
            result = await run.result(tool, "verify", "--keys-dir", keys, package)
            if result.exit_code:
                raise TaskError(
                    f"{TASK}: APK signature verification failed: {package.name}"
                )
        index = root / "APKINDEX.tar.gz"
        await run(
            tool,
            "--allow-untrusted",
            "index",
            "--description",
            f"Dimidium Labs {context.service} {context.channel}",
            "--output",
            index,
            sorted(root.glob("*.apk")),
        )
        await sign_index(context, index)
        context.storage.upload_payloads(root, remote, "*.apk")
        context.storage.upload(index, f"{remote}/APKINDEX.tar.gz")
