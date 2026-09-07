#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0
# fmt: off
#MISE description="Finalize an already published GitHub release"
#MISE tools={"pipx"="1.16.7","python"="3.14.7","gh"="2.100.0"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import json
import re
import sys
from collections.abc import Sequence
from pathlib import Path

sys.dont_write_bytecode = True

from libs.common import (
    TaskError,
    capture,
    controlled_environment,
    require_command,
    run,
    task_main,
)

TASK = "github-release"
SAFE_REPOSITORY = re.compile(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$")
SAFE_REVISION = re.compile(r"^(?:[0-9A-Fa-f]{40}|[0-9A-Fa-f]{64})$")
SAFE_TAG = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._+-]*$")
PROVIDER_ENVIRONMENT = (
    "DOCKER_CERT_PATH",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    "DOCKER_HOST",
    "DOCKER_TLS_VERIFY",
    "GH_CONFIG_DIR",
    "GH_ENTERPRISE_TOKEN",
    "GH_HOST",
    "GH_TOKEN",
    "NO_COLOR",
)


def release_notes(channel: str, version: str, checks_url: str) -> str:
    if channel == "nightly":
        heading = "Last successful build from `main` branch."
    else:
        heading = f"Release {version}."
    lines = [heading, "", f"**Version**: {version}"]
    if checks_url:
        lines.extend(("", f"**Release checks**: {checks_url}"))
    return "\n".join(lines)


async def existing_assets(
    environment: dict[str, str], repository: str, tag: str
) -> list[str] | None:
    result = await capture.result.set(env=environment, inherit_env=False)(
        "gh",
        "release",
        "view",
        tag,
        "--repo",
        repository,
        "--json",
        "assets",
    )
    if result.exit_code:
        return None
    try:
        document = json.loads(result.output)
        assets = document["assets"]
        names = [asset["name"] for asset in assets]
    except (KeyError, TypeError, json.JSONDecodeError) as error:
        raise TaskError(f"{TASK}: invalid GitHub release response") from error
    if not all(isinstance(name, str) and name for name in names):
        raise TaskError(f"{TASK}: invalid GitHub release asset name")
    return names


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(prog="mise run github-release --")
    command.add_argument("--repository", required=True)
    command.add_argument("--channel", required=True, choices=("nightly", "stable"))
    command.add_argument("--tag", required=True)
    command.add_argument("--title", required=True)
    command.add_argument("--revision", required=True)
    command.add_argument("--version", required=True)
    command.add_argument("--checks-url", default="")
    command.add_argument(
        "--asset",
        action="extend",
        nargs="+",
        required=True,
        default=[],
        type=Path,
    )
    command.add_argument("--image")
    command.add_argument("--image-alias")
    arguments = command.parse_args(args)

    if not SAFE_REPOSITORY.fullmatch(arguments.repository):
        command.error(f"invalid repository: {arguments.repository}")
    if not SAFE_TAG.fullmatch(arguments.tag):
        command.error(f"invalid release tag: {arguments.tag}")
    if not arguments.title or "\n" in arguments.title:
        command.error("release title must be non-empty and single-line")
    if not SAFE_REVISION.fullmatch(arguments.revision):
        command.error(f"invalid revision: {arguments.revision}")
    if not arguments.version or "\n" in arguments.version:
        command.error("version must be non-empty and single-line")
    if "\n" in arguments.checks_url:
        command.error("checks URL must be single-line")
    if bool(arguments.image) != bool(arguments.image_alias):
        command.error("--image and --image-alias must be supplied together")
    if any(not asset.is_file() for asset in arguments.asset):
        missing = next(asset for asset in arguments.asset if not asset.is_file())
        raise TaskError(f"{TASK}: asset not found: {missing}")
    names = [asset.name for asset in arguments.asset]
    if len(names) != len(set(names)):
        command.error("release asset names must be unique")

    require_command("gh", TASK)
    if arguments.image:
        require_command("docker", TASK)
    environment = controlled_environment(PROVIDER_ENVIRONMENT)
    execute = run.set(env=environment, inherit_env=False)
    notes = release_notes(arguments.channel, arguments.version, arguments.checks_url)

    assets = await existing_assets(environment, arguments.repository, arguments.tag)
    if assets is None:
        create: list[str | Path] = [
            "gh",
            "release",
            "create",
            arguments.tag,
            "--repo",
            arguments.repository,
            "--title",
            arguments.title,
            "--notes",
            notes,
        ]
        if arguments.channel == "nightly":
            create.extend(("--target", arguments.revision, "--prerelease"))
        else:
            create.append("--verify-tag")
        await execute(create)
    else:
        for asset in assets:
            await execute(
                "gh",
                "release",
                "delete-asset",
                arguments.tag,
                asset,
                "--repo",
                arguments.repository,
                "--yes",
            )
        edit: list[str] = [
            "gh",
            "release",
            "edit",
            arguments.tag,
            "--repo",
            arguments.repository,
            "--title",
            arguments.title,
            "--notes",
            notes,
        ]
        if arguments.channel == "nightly":
            edit.append("--prerelease")
        await execute(edit)

    await execute(
        "gh",
        "release",
        "upload",
        arguments.tag,
        arguments.asset,
        "--repo",
        arguments.repository,
    )

    if arguments.image:
        await execute(
            "docker",
            "buildx",
            "imagetools",
            "create",
            "--tag",
            arguments.image_alias,
            arguments.image,
        )

    if arguments.channel == "nightly":
        reference = f"repos/{arguments.repository}/git/ref/tags/{arguments.tag}"
        result = await capture.result.set(env=environment, inherit_env=False)(
            "gh", "api", reference
        )
        if result.exit_code == 0:
            await execute(
                "gh",
                "api",
                "--method",
                "PATCH",
                f"repos/{arguments.repository}/git/refs/tags/{arguments.tag}",
                "-f",
                f"sha={arguments.revision}",
                "-F",
                "force=true",
            )
        else:
            await execute(
                "gh",
                "api",
                "--method",
                "POST",
                f"repos/{arguments.repository}/git/refs",
                "-f",
                f"ref=refs/tags/{arguments.tag}",
                "-f",
                f"sha={arguments.revision}",
            )


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
