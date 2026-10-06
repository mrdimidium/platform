#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# fmt: off
#MISE description="Resolve a CI-neutral Rust service release context"
#MISE tools={"pipx"="1.16.7","python"="3.14.7"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import json
import re
import shlex
import sys
from collections.abc import Sequence
from pathlib import Path

sys.dont_write_bytecode = True

from libs.common import TaskError, capture, require_command, task_main

TASK = "release-context"
SAFE_REVISION = re.compile(r"^(?:[0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$")
SAFE_TIMESTAMP = re.compile(r"^[0-9]+$")
SAFE_BUILD_NUMBER = re.compile(r"^[A-Za-z0-9]+(?:[.-][A-Za-z0-9]+)*$")


async def package_version(package: str) -> str:
    require_command("cargo", TASK)
    metadata = json.loads(
        await capture("cargo", "metadata", "--no-deps", "--format-version", "1")
    )
    versions = {
        item["version"] for item in metadata["packages"] if item["name"] == package
    }
    if len(versions) != 1:
        raise TaskError(f"{TASK}: cannot determine {package} version")
    return versions.pop()


async def git_value(*arguments: str) -> str:
    require_command("git", TASK)
    value = (await capture("git", arguments)).strip()
    if not value:
        raise TaskError(f"{TASK}: git returned an empty value")
    return value


def write_environment(output: Path, values: Sequence[tuple[str, str]]) -> None:
    lines = [f"{name}={shlex.quote(value)}" for name, value in values]
    lines.append(f"export SOURCE_DATE_EPOCH={shlex.quote(dict(values)['timestamp'])}")
    output.write_text("\n".join(lines) + "\n")


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(prog="mise run release-context --")
    command.add_argument("--package", required=True)
    command.add_argument("--source-ref", default="")
    command.add_argument("--nightly-ref", default="refs/heads/main")
    command.add_argument("--revision", default="")
    command.add_argument("--build-number", default="local")
    command.add_argument("--publish", action="store_true")
    command.add_argument("--output", required=True, type=Path)
    arguments = command.parse_args(args)

    if not SAFE_BUILD_NUMBER.fullmatch(arguments.build_number):
        raise TaskError(f"{TASK}: invalid build number: {arguments.build_number}")

    base_version = await package_version(arguments.package)
    revision = arguments.revision or await git_value("rev-parse", "HEAD")
    if not SAFE_REVISION.fullmatch(revision):
        raise TaskError(f"{TASK}: invalid revision: {revision}")
    timestamp = await git_value("show", "-s", "--format=%ct", revision, "--")
    if not SAFE_TIMESTAMP.fullmatch(timestamp):
        raise TaskError(f"{TASK}: invalid commit timestamp: {timestamp}")

    source_ref = arguments.source_ref
    if source_ref.startswith("refs/tags/v"):
        version = source_ref.removeprefix("refs/tags/v")
        if version != base_version:
            raise TaskError(
                f"{TASK}: tag version {version} does not match "
                f"Cargo.toml version {base_version}"
            )
        package_release_version = version
        channel = "stable"
        release_tag = f"v{version}"
        release_name = version
    elif source_ref == arguments.nightly_ref:
        version = f"{base_version}-nightly.{timestamp}"
        package_release_version = f"{base_version}~nightly.{timestamp}"
        channel = "nightly"
        release_tag = "nightly"
        release_name = "nightly"
    else:
        version = f"{base_version}-pr.{arguments.build_number}"
        package_release_version = f"{base_version}~pr.{arguments.build_number}"
        channel = ""
        release_tag = ""
        release_name = ""

    if (
        arguments.publish
        and source_ref != arguments.nightly_ref
        and not source_ref.startswith("refs/tags/v")
    ):
        raise TaskError(f"{TASK}: ref is not publishable: {source_ref or '<unset>'}")

    write_environment(
        arguments.output,
        (
            ("base_version", base_version),
            ("revision", revision),
            ("timestamp", timestamp),
            ("version", version),
            ("package_version", package_release_version),
            ("channel", channel),
            ("release_tag", release_tag),
            ("release_name", release_name),
            ("publish", str(arguments.publish).lower()),
        ),
    )


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
