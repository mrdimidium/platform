# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

import shutil
from collections.abc import Mapping, Sequence

from .common import TaskError, require_command, run

DependencyMap = Mapping[str, Mapping[str, Sequence[str]]]


def package_manager(task: str) -> str:
    if shutil.which("apt-get"):
        return "apt"
    if shutil.which("dnf"):
        return "dnf"
    raise TaskError(f"{task}: no supported system package manager found")


async def bootstrap(
    task: str,
    capabilities: Sequence[str],
    dependencies: DependencyMap,
    *,
    update: bool = False,
    dry_run: bool = False,
) -> None:
    require_command("mise", task)
    manager = package_manager(task)
    available = dependencies[manager]
    unsupported = sorted(set(capabilities) - set(available))
    if unsupported:
        names = ", ".join(unsupported)
        raise TaskError(f"{task}: {names} bootstrap is unsupported with {manager}")

    packages = sorted(
        {package for capability in capabilities for package in available[capability]}
    )
    arguments = ["mise", "bootstrap", "packages", "apply", "--yes"]
    if update:
        arguments.append("--update")
    if dry_run:
        arguments.append("--dry-run")
    arguments.extend(f"{manager}:{package}" for package in packages)
    await run(arguments)
