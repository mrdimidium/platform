#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import asyncio
import os
import shlex
import shutil
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path

from shellous import sh

run = sh.stdout(sh.INHERIT).stderr(sh.INHERIT)


def executable(path: Path, source: str) -> None:
    path.write_text("#!/usr/bin/env python3\n" + source)
    path.chmod(0o755)


def environment_file(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    for line in path.read_text().splitlines():
        if line.startswith("export "):
            line = line.removeprefix("export ")
        name, raw_value = line.split("=", 1)
        values[name] = shlex.split(raw_value)[0] if raw_value != "''" else ""
    return values


async def main(args: Sequence[str]) -> None:
    if args:
        raise SystemExit(f"unexpected arguments: {' '.join(args)}")
    root = Path(__file__).resolve().parent.parent
    task = root / "tasks/release-context.py"
    assert "GITHUB_" not in task.read_text()

    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        binary = work / "bin"
        binary.mkdir()
        executable(
            binary / "cargo",
            """import json
import sys
if sys.argv[1:] != ["metadata", "--no-deps", "--format-version", "1"]:
    raise SystemExit(1)
print(json.dumps({"packages": [{"name": "service", "version": "1.2.3"}]}))
""",
        )
        executable(
            binary / "git",
            """import sys
arguments = sys.argv[1:]
revisions = {
    "0123456789abcdef0123456789abcdef01234567",
    "fedcba9876543210fedcba9876543210fedcba98",
}
if arguments == ["rev-parse", "HEAD"]:
    print("0123456789abcdef0123456789abcdef01234567")
elif (
    len(arguments) == 5
    and arguments[:3] == ["show", "-s", "--format=%ct"]
    and arguments[3] in revisions
    and arguments[4] == "--"
):
    print("1700000000")
else:
    raise SystemExit(1)
""",
        )

        environment = dict(os.environ)
        environment["PATH"] = f"{binary}:{environment['PATH']}"
        command = run.set(env=environment, inherit_env=False)

        nightly = work / "nightly.env"
        await command(
            task,
            "--package",
            "service",
            "--source-ref",
            "refs/heads/main",
            "--build-number",
            "42",
            "--output",
            nightly,
        )
        values = environment_file(nightly)
        assert values["base_version"] == "1.2.3"
        assert values["revision"] == "0123456789abcdef0123456789abcdef01234567"
        assert values["SOURCE_DATE_EPOCH"] == "1700000000"
        assert values["version"] == "1.2.3-nightly.1700000000"
        assert values["package_version"] == "1.2.3~nightly.1700000000"
        assert values["channel"] == "nightly"
        assert values["publish"] == "false"

        tag = work / "tag.env"
        await command(
            task,
            "--package",
            "service",
            "--source-ref",
            "refs/tags/v1.2.3",
            "--revision",
            "fedcba9876543210fedcba9876543210fedcba98",
            "--output",
            tag,
        )
        values = environment_file(tag)
        assert values["version"] == "1.2.3"
        assert values["package_version"] == "1.2.3"
        assert values["channel"] == "stable"
        assert values["release_tag"] == "v1.2.3"

        published = work / "published.env"
        await command(
            task,
            "--package",
            "service",
            "--source-ref",
            "refs/heads/main",
            "--publish",
            "--output",
            published,
        )
        assert environment_file(published)["publish"] == "true"

        result = await command.result(
            task,
            "--package",
            "service",
            "--source-ref",
            "refs/heads/feature",
            "--publish",
            "--output",
            work / "feature.env",
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0

        real_git = shutil.which("git", path=os.defpath)
        assert real_git is not None
        (binary / "git").unlink()
        repository = work / "repository"
        repository.mkdir()
        await run(real_git, "-C", repository, "init", "--quiet")
        await run(real_git, "-C", repository, "config", "user.name", "Fixture")
        await run(
            real_git,
            "-C",
            repository,
            "config",
            "user.email",
            "fixture@example.invalid",
        )
        (repository / "tracked").write_text("fixture\n")
        commit_environment = dict(
            os.environ,
            GIT_AUTHOR_DATE="1700000123 +0000",
            GIT_COMMITTER_DATE="1700000123 +0000",
        )
        await run.set(env=commit_environment, inherit_env=False)(
            real_git, "-C", repository, "add", "tracked"
        )
        await run.set(env=commit_environment, inherit_env=False)(
            real_git, "-C", repository, "commit", "--quiet", "-m", "fixture"
        )
        revision = (await sh(real_git, "-C", repository, "rev-parse", "HEAD")).strip()
        real_environment = dict(environment)
        real_environment["PATH"] = f"{binary}:{os.defpath}"
        real_output = work / "real.env"
        await run.set(cwd=repository, env=real_environment, inherit_env=False)(
            sys.executable,
            task,
            "--package",
            "service",
            "--source-ref",
            "refs/heads/main",
            "--revision",
            revision,
            "--output",
            real_output,
        )
        assert environment_file(real_output)["timestamp"] == "1700000123"

    print("release context: ok")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
