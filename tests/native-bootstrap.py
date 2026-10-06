#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["boto3==1.43.75", "shellous==0.42.0"]
# ///

from __future__ import annotations

import asyncio
import os
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path

from shellous import sh

run = sh.stdout(sh.INHERIT).stderr(sh.INHERIT)


def executable(path: Path, source: str = "#!/bin/sh\nexit 0\n") -> None:
    path.write_text(source)
    path.chmod(0o755)


async def main(args: Sequence[str]) -> None:
    if args:
        raise SystemExit(f"unexpected arguments: {' '.join(args)}")
    root = Path(__file__).resolve().parent.parent
    package = root / "tasks/package.py"
    publish = root / "tasks/publish.py"

    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        binary = work / "bin"
        binary.mkdir()
        log = work / "mise.log"
        executable(
            binary / "mise",
            '#!/bin/sh\nprintf "%s\\n" "$@" >"$NATIVE_TEST_LOG"\n',
        )
        environment = dict(
            os.environ,
            PATH=str(binary),
            NATIVE_TEST_LOG=str(log),
            PUBLISH_TEST_PYTHONPATH=str(root / "tests/fakes"),
        )
        command = run.set(env=environment, inherit_env=False)

        executable(binary / "apt-get")
        await command(
            sys.executable,
            package,
            "--bootstrap",
            "--update",
            "deb",
            "rpm",
            "apk",
            "tar.gz",
            "zip",
        )
        arguments = log.read_text().splitlines()
        assert arguments[:5] == [
            "bootstrap",
            "packages",
            "apply",
            "--yes",
            "--update",
        ]
        for dependency in (
            "apt:debsigs",
            "apt:gnupg",
            "apt:gzip",
            "apt:openssl",
            "apt:rpm",
            "apt:tar",
            "apt:zip",
        ):
            assert dependency in arguments
        assert "apt:build-essential" not in arguments
        assert not any(argument.startswith("dnf:") for argument in arguments)

        await command(
            sys.executable,
            publish,
            "--bootstrap",
            "--dry-run",
            "deb",
            "rpm",
            "apk",
        )
        arguments = log.read_text().splitlines()
        assert arguments[:5] == [
            "bootstrap",
            "packages",
            "apply",
            "--yes",
            "--dry-run",
        ]
        for dependency in (
            "apt:apt-utils",
            "apt:createrepo-c",
            "apt:dpkg",
            "apt:gnupg",
            "apt:openssl",
        ):
            assert dependency in arguments

        (binary / "apt-get").unlink()
        executable(binary / "dnf")
        await command(
            sys.executable,
            package,
            "--bootstrap",
            "rpm",
            "apk",
            "zip",
        )
        arguments = log.read_text().splitlines()
        for dependency in (
            "dnf:gnupg2",
            "dnf:openssl",
            "dnf:rpm",
            "dnf:rpm-sign",
            "dnf:zip",
        ):
            assert dependency in arguments
        assert not any(argument.startswith("apt:") for argument in arguments)

        result = await command.result(
            sys.executable, package, "--bootstrap", "deb"
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0
        result = await command.result(
            sys.executable, publish, "--bootstrap", "deb"
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0
        result = await command.result(
            sys.executable, package, "--update", "zip"
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0

        (binary / "dnf").unlink()
        result = await command.result(
            sys.executable, package, "--bootstrap", "zip"
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0

    print("native bootstrap: ok")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
