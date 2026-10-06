# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

import re
import sys
from pathlib import Path

sys.dont_write_bytecode = True

from .common import TaskError, capture, run

TASK = "check"
HEADER = (
    r"^((<!--|#|//|/\*|\*)[[:space:]]*)?"
    r"(Copyright[[:space:]]+(\([cC]\)|©)|SPDX-FileCopyrightText:)"
)
LEGACY_COPYRIGHT = re.compile(r"^\s*((<!--|#|//|/\*|\*)\s*)?Copyright\s+(\([cC]\)|©)")
CANONICAL_COPYRIGHT = re.compile(
    r"SPDX-FileCopyrightText: 2026 Nikolay Govorov(?:\s*(?:\*/|-->))?$"
)


async def check_copyright_headers() -> None:
    result = await capture.result(
        "git",
        "grep",
        "-n",
        "-I",
        "-E",
        HEADER,
        "--",
        ".",
        ":(exclude)*.md",
        ":(exclude)LICENSE",
        ":(exclude)LICENSES/**",
        ":(exclude)COPYING*",
    )
    if result.exit_code not in {0, 1}:
        raise TaskError(f"{TASK}: git grep failed with exit code {result.exit_code}")

    invalid: list[str] = []
    for line in result.output.splitlines():
        match = re.search(r":([0-9]+):", line)
        if match is None or int(match.group(1)) > 10:
            continue
        text = line[match.end() :]
        reason = ""
        if LEGACY_COPYRIGHT.match(text):
            reason = "legacy copyright header"
        elif (position := text.find("SPDX-FileCopyrightText:")) >= 0:
            suffix = text[position + len("SPDX-FileCopyrightText:") :]
            if (
                not suffix
                or not suffix.startswith(" ")
                or (len(suffix) > 1 and suffix[1].isspace())
            ):
                reason = "expected exactly one space after colon"
        if (
            not reason
            and "SPDX-FileCopyrightText:" in text
            and "Nikolay Govorov" in text
            and not CANONICAL_COPYRIGHT.search(text)
        ):
            reason = "expected 2026 Nikolay Govorov"
        if reason:
            invalid.append(f"{reason}: {line}")

    if invalid:
        print("Invalid copyright headers:", *invalid, sep="\n", file=sys.stderr)
        raise TaskError(f"{TASK}: invalid copyright headers")


async def check() -> None:
    await check_copyright_headers()
    await run("reuse", "lint")
    if Path("Cargo.toml").is_file():
        await run("cargo-deny", "check")
