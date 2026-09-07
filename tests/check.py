#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import asyncio
import os
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path

from shellous import sh

run = sh.stdout(sh.INHERIT).stderr(sh.INHERIT)


def executable(path: Path, source: str) -> None:
    path.write_text("#!/usr/bin/env python3\n" + source)
    path.chmod(0o755)


async def main(args: Sequence[str]) -> None:
    if args:
        raise SystemExit(f"unexpected arguments: {' '.join(args)}")
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        binary = work / "bin"
        project = work / "project"
        target = project / "target"
        binary.mkdir()
        target.mkdir(parents=True)
        (project / "CLA.md").write_text("Version 1.0\n")
        (project / ".mailmap").write_text("")
        (project / "Cargo.toml").write_text("[workspace]\n")
        (project / "Mirumfile").write_text("#!/bin/sh\ntrue\n")
        (project / "deploy.sh").write_text("#!/bin/sh\ntrue\n")
        (project / "service.initd").write_text("#!/bin/sh\ntrue\n")
        (target / "ignored.sh").write_text("#!/bin/sh\nfalse\n")
        project_test = project / "project-test"
        executable(
            project_test,
            """import os
from pathlib import Path
with Path(os.environ["CHECK_TEST_LOG"]).open("a") as stream:
    stream.write("project-test\\n")
""",
        )
        executable(
            binary / "git",
            f"""import os
import sys
from pathlib import Path
arguments = sys.argv[1:]
with Path(os.environ["CHECK_TEST_LOG"]).open("a") as stream:
    stream.write(f"git {{' '.join(arguments)}}\\n")
if arguments == ["rev-parse", "--show-toplevel"]:
    print({str(project)!r})
elif arguments and arguments[0] == "grep":
    raise SystemExit(1)
elif arguments[:2] == ["log", "--no-merges"]:
    pass
else:
    raise SystemExit(1)
""",
        )
        for name in ("cargo", "cargo-deny", "reuse", "shellcheck"):
            executable(
                binary / name,
                """import os
import sys
from pathlib import Path
with Path(os.environ["CHECK_TEST_LOG"]).open("a") as stream:
    stream.write(f"{Path(sys.argv[0]).name} {' '.join(sys.argv[1:])}\\n")
""",
            )

        log = work / "check.log"
        environment = dict(os.environ)
        environment.update(
            PATH=f"{binary}:{environment['PATH']}",
            CHECK_TEST_LOG=str(log),
        )
        command = run.set(cwd=project, env=environment, inherit_env=False)
        task = root / "tasks/check.py"

        await command(task, "--script", project_test)
        lines = log.read_text().splitlines()
        for expected in (
            "reuse lint",
            "cargo-deny check",
            "cargo fmt --all --check",
            "cargo clippy --locked --workspace --all-targets --all-features --release -- -D warnings",
            "cargo test --workspace --all-features --release --locked",
            "project-test",
            "cargo llvm-cov --all-features --workspace --lcov --output-path target/coverage/lcov.info",
        ):
            assert expected in lines
        shellcheck = next(line for line in lines if line.startswith("shellcheck "))
        for source in ("Mirumfile", "deploy.sh", "service.initd"):
            assert source in shellcheck
        assert "ignored.sh" not in shellcheck

        log.write_text("")
        await command(task, "policy")
        policy_lines = log.read_text().splitlines()
        assert "reuse lint" in policy_lines
        assert "cargo-deny check" in policy_lines
        assert not any(line.startswith("cargo ") for line in policy_lines)
        assert not any(line.startswith("shellcheck ") for line in policy_lines)

        result = await command.result(task, "policy", "--script", project_test).stderr(
            sh.DEVNULL
        )
        assert result.exit_code != 0

    print("check: ok")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
