#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# fmt: off
#MISE description="Run shared repository policy and Rust checks"
#MISE tools={"pipx"="1.16.7","python"="3.14.7","shellcheck"="0.11.0","aqua:taiki-e/cargo-llvm-cov"="0.8.7","pipx:reuse"="6.2.0","aqua:EmbarkStudios/cargo-deny"="0.19.0"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Sequence
from pathlib import Path

sys.dont_write_bytecode = True

from libs import licenses
from libs.common import TaskError, require_command, run, task_main

TASK = "check"
EXCLUDED_DIRECTORIES = {".git", ".container", "dist", "target"}


def shell_sources(root: Path) -> list[Path]:
    sources: list[Path] = []
    for directory, names, files in os.walk(root):
        names[:] = [name for name in names if name not in EXCLUDED_DIRECTORIES]
        parent = Path(directory)
        for name in files:
            if name == "Mirumfile" or Path(name).suffix in {".initd", ".sh"}:
                sources.append(parent / name)
    return sorted(sources)


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(prog="mise run check --")
    command.add_argument(
        "scope",
        nargs="?",
        choices=("all", "policy"),
        default="all",
        help="run all checks (default) or organizational policy only",
    )
    command.add_argument(
        "--script",
        action="append",
        default=[],
        type=Path,
        help="additional executable project test; may be repeated",
    )
    arguments = command.parse_args(args)

    if arguments.scope == "policy" and arguments.script:
        command.error("--script is only available with the all scope")

    await licenses.check()
    if arguments.scope == "policy":
        return

    require_command("cargo", TASK)
    require_command("shellcheck", TASK)
    for script in arguments.script:
        if not script.is_file():
            raise TaskError(f"{TASK}: test script not found: {script}")
        if not os.access(script, os.X_OK):
            raise TaskError(f"{TASK}: test script is not executable: {script}")

    await run("cargo", "fmt", "--all", "--check")

    sources = shell_sources(Path("."))
    if sources:
        await run("shellcheck", sources)

    await run(
        "cargo",
        "clippy",
        "--locked",
        "--workspace",
        "--all-targets",
        "--all-features",
        "--release",
        "--",
        "-D",
        "warnings",
    )
    await run("cargo", "test", "--workspace", "--all-features", "--release", "--locked")
    for script in arguments.script:
        await run(script)

    coverage = Path("target/coverage")
    coverage.mkdir(parents=True, exist_ok=True)
    await run(
        "cargo",
        "llvm-cov",
        "--all-features",
        "--workspace",
        "--lcov",
        "--output-path",
        coverage / "lcov.info",
    )


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
