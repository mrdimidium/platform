#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# fmt: off
#MISE description="Lint, package, and optionally publish a Helm chart"
#MISE tools={"pipx"="1.16.7","python"="3.14.7","helm"="4.1.1"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Sequence
from pathlib import Path

sys.dont_write_bytecode = True

from libs.common import TaskError, require_command, run, task_main

TASK = "chart"
CHART_NAME = re.compile(r"^name:\s*([A-Za-z0-9_.-][A-Za-z0-9_.-]*)\s*$")


def chart_name(chart: Path) -> str:
    for line in (chart / "Chart.yaml").read_text().splitlines():
        if match := CHART_NAME.fullmatch(line):
            return match.group(1)
    raise TaskError(f"{TASK}: cannot read chart name from {chart}/Chart.yaml")


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(prog="mise run chart --")
    command.add_argument("--chart", required=True, type=Path)
    command.add_argument("--version")
    command.add_argument("--app-version")
    command.add_argument("--output", default=Path("dist/charts"), type=Path)
    command.add_argument("--push", action="append", default=[], metavar="OCI_URL")
    command.add_argument("--lint-only", action="store_true")
    arguments = command.parse_args(args)

    require_command("helm", TASK)
    if not arguments.chart.is_dir():
        raise TaskError(f"{TASK}: directory not found: {arguments.chart}")
    if not (arguments.chart / "Chart.yaml").is_file():
        raise TaskError(f"{TASK}: Chart.yaml not found in {arguments.chart}")
    for registry in arguments.push:
        if not registry.startswith("oci://"):
            command.error(f"registry must use oci://: {registry}")

    name = chart_name(arguments.chart)
    await run("helm", "lint", arguments.chart, "--strict")

    if arguments.lint_only:
        if arguments.version or arguments.app_version or arguments.push:
            command.error("--lint-only cannot package or push a chart")
        return
    if not arguments.version:
        command.error("--version is required unless --lint-only is used")

    arguments.output.mkdir(parents=True, exist_ok=True)
    package_arguments: list[str | Path] = [
        "helm",
        "package",
        arguments.chart,
        "--destination",
        arguments.output,
        "--version",
        arguments.version,
    ]
    if arguments.app_version:
        package_arguments.extend(("--app-version", arguments.app_version))
    await run(package_arguments)

    package = arguments.output / f"{name}-{arguments.version}.tgz"
    if not package.is_file():
        raise TaskError(f"{TASK}: Helm did not create expected package: {package}")
    for registry in arguments.push:
        await run("helm", "push", package, registry.rstrip("/"))


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
