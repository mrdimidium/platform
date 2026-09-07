#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0
# fmt: off
#MISE description="Build and optionally publish an OCI container image"
#MISE tools={"pipx"="1.16.7","python"="3.14.7"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence
from pathlib import Path

sys.dont_write_bytecode = True

from libs.common import TaskError, require_command, run, task_main

TASK = "container"


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(prog="mise run container --")
    command.add_argument("--context", default=Path("."), type=Path)
    command.add_argument("--file", default=Path("Dockerfile"), type=Path)
    command.add_argument("--platform", default="linux/amd64")
    command.add_argument("--target")
    command.add_argument("--build-arg", action="append", default=[])
    command.add_argument("--label", action="append", default=[])
    command.add_argument("--tag", action="append", required=True)
    command.add_argument("--cache-from", action="append", default=[])
    command.add_argument("--cache-to", action="append", default=[])
    command.add_argument("--provenance", choices=("true", "false"), default="true")
    command.add_argument("--sbom", choices=("true", "false"), default="true")
    command.add_argument("--push", action="store_true")
    command.add_argument("--load", action="store_true")
    arguments = command.parse_args(args)

    require_command("docker", TASK)
    if not arguments.context.is_dir():
        raise TaskError(f"{TASK}: context directory not found: {arguments.context}")
    dockerfile = (
        arguments.file
        if arguments.file.is_absolute()
        else arguments.context / arguments.file
    )
    if not dockerfile.is_file():
        raise TaskError(f"{TASK}: Dockerfile not found: {dockerfile}")
    for value in arguments.build_arg:
        if "\n" in value:
            command.error("build arguments cannot contain newlines")
    for value in arguments.label:
        if "\n" in value:
            command.error("labels cannot contain newlines")
    for tag in arguments.tag:
        if not tag or any(character.isspace() for character in tag):
            command.error(f"invalid image tag: {tag}")
    if arguments.push and arguments.load:
        command.error("--push and --load are mutually exclusive")
    if arguments.load and "," in arguments.platform:
        command.error("--load supports exactly one platform")

    build: list[str | Path] = [
        "docker",
        "buildx",
        "build",
        "--file",
        dockerfile,
        "--platform",
        arguments.platform,
        f"--provenance={arguments.provenance}",
        f"--sbom={arguments.sbom}",
    ]
    if arguments.target:
        build.extend(("--target", arguments.target))
    for value in arguments.build_arg:
        build.extend(("--build-arg", value))
    for value in arguments.label:
        build.extend(("--label", value))
    for tag in arguments.tag:
        build.extend(("--tag", tag))
    for cache in arguments.cache_from:
        build.extend(("--cache-from", cache))
    for cache in arguments.cache_to:
        build.extend(("--cache-to", cache))
    if arguments.push:
        build.append("--push")
    elif arguments.load:
        build.append("--load")
    build.append(arguments.context)

    await run(*build)


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
