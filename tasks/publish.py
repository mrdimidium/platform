#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0
# fmt: off
#MISE description="Publish signed package repositories to shared S3 storage"
#MISE tools={"pipx"="1.16.7","python"="3.14.7"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["boto3==1.43.75", "shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path

sys.dont_write_bytecode = True

from libs.common import TaskError, task_main
from libs.native import bootstrap as bootstrap_native

if test_path := os.environ.get("PUBLISH_TEST_PYTHONPATH"):
    sys.path.insert(0, test_path)

from libs.repository import Repository

TASK = "publish"
FORMATS = {"deb", "rpm", "apk"}
SAFE_SLUG = re.compile(r"^[a-z0-9](?:[a-z0-9-]*[a-z0-9])?$")
NATIVE_DEPENDENCIES = {
    "apt": {
        "apk": ("openssl", "tar"),
        "deb": ("apt-utils", "dpkg", "gnupg", "gzip"),
        "rpm": ("createrepo-c", "gnupg", "rpm"),
    },
    "dnf": {
        "apk": ("openssl", "tar"),
        "rpm": ("createrepo_c", "gnupg2", "rpm"),
    },
}


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(
        prog="mise run publish --",
        usage=(
            "%(prog)s [--bootstrap [--update|--dry-run]] "
            "[--service NAME --channel CHANNEL --input DIR] deb|rpm|apk..."
        ),
    )
    command.add_argument(
        "--bootstrap",
        action="store_true",
        help="install native dependencies for the requested formats and exit",
    )
    command.add_argument("--update", action="store_true")
    command.add_argument("--dry-run", action="store_true")
    command.add_argument("--service")
    command.add_argument("--channel")
    command.add_argument("--input", type=Path)
    command.add_argument("formats", nargs="+", choices=sorted(FORMATS))
    arguments = command.parse_args(args)
    if arguments.update and arguments.dry_run:
        command.error("--update and --dry-run are mutually exclusive")
    if arguments.bootstrap:
        await bootstrap_native(
            TASK,
            arguments.formats,
            NATIVE_DEPENDENCIES,
            update=arguments.update,
            dry_run=arguments.dry_run,
        )
        return
    if arguments.update or arguments.dry_run:
        command.error("--update and --dry-run require --bootstrap")
    for name in ("service", "channel", "input"):
        if getattr(arguments, name) is None:
            command.error(f"--{name} is required unless --bootstrap is used")
    for label, value in (
        ("service name", arguments.service),
        ("channel", arguments.channel),
    ):
        if not SAFE_SLUG.fullmatch(value) or "--" in value:
            raise TaskError(f"{TASK}: invalid {label}: {value}")
    if not arguments.input.is_dir():
        raise TaskError(f"{TASK}: {arguments.input} not found")
    with tempfile.TemporaryDirectory(prefix="publish-") as directory:
        await Repository(
            arguments.service,
            arguments.channel,
            arguments.input.resolve(),
            arguments.formats,
            Path(directory),
        ).publish()


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
