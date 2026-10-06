#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import asyncio
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path

from shellous import sh

run = sh.stdout(sh.INHERIT).stderr(sh.INHERIT)
capture = sh.stderr(sh.INHERIT)


async def main(args: Sequence[str]) -> None:
    if args:
        raise SystemExit(f"unexpected arguments: {' '.join(args)}")
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        stage = work / "stage"
        stage.mkdir()
        (stage / "package-contract").write_text("package contract\n")
        config = work / "nfpm.yaml"
        config.write_text(
            """name: package-contract
arch: ${ARCH}
version: ${VERSION}
platform: linux
maintainer: Dimidium Labs <me@govorov.online>
description: Shared package task integration fixture
license: 0BSD
"""
        )
        output = work / "out"
        await run(
            "mise",
            "--cd",
            root,
            "run",
            "package",
            "--",
            "--config",
            config,
            "--version",
            "1.2.3~nightly.42",
            "--arch",
            "amd64",
            "--output",
            output,
            "--archive-root",
            stage,
            "--archive-name",
            "package-contract-linux-amd64",
            "deb",
            "rpm",
            "apk",
            "tar.gz",
            "zip",
        )

        assert len(list(output.glob("*.deb"))) == 1
        assert len(list(output.glob("*.rpm"))) == 1
        assert len(list(output.glob("*.apk"))) == 1
        tarball = output / "package-contract-linux-amd64.tar.gz"
        zipfile = output / "package-contract-linux-amd64.zip"
        assert tarball.is_file()
        assert zipfile.is_file()

        package = next(output.glob("*.deb"))
        assert (await capture("dpkg-deb", "-f", package, "Package")).strip() == (
            "package-contract"
        )
        assert (await capture("dpkg-deb", "-f", package, "Version")).strip() == (
            "1.2.3~nightly.42"
        )
        assert (await capture("dpkg-deb", "-f", package, "Architecture")).strip() == (
            "amd64"
        )
        assert "./package-contract" in await capture("tar", "-tzf", tarball)
        assert "package-contract" in await capture("unzip", "-l", zipfile)


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
