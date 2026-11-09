# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0

from __future__ import annotations

import os
import re
from pathlib import Path

from .common import TaskError, capture


async def check() -> None:
    root = Path((await capture("git", ("rev-parse", "--show-toplevel"))).strip())
    os.chdir(root)

    mailmap = Path(".mailmap")
    if not mailmap.is_file():
        raise TaskError("Contributor registry .mailmap is missing")
    lines = [
        line
        for line in mailmap.read_text().splitlines()
        if not re.match(r"^\s*#", line)
    ]
    history = await capture("git", ("log", "--no-merges", "--format=%ae%n%ce"))
    emails = sorted(set(history.splitlines()))
    missing = False
    for email in emails:
        if not any(f"<{email}>" in line for line in lines):
            print(f"Email <{email}> is not in .mailmap")
            missing = True
    if missing:
        raise TaskError("All authors and committers must be listed in .mailmap")
