#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import asyncio
import hashlib
import json
import os
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


def entries(path: Path) -> list[list[str]]:
    return [json.loads(line) for line in path.read_text().splitlines()]


async def main(args: Sequence[str]) -> None:
    if args:
        raise SystemExit(f"unexpected arguments: {' '.join(args)}")
    root = Path(__file__).resolve().parent.parent
    task = root / "tasks/github-release.py"
    assert "GITHUB_" not in task.read_text()

    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        binary = work / "bin"
        state = work / "state"
        log = work / "commands.jsonl"
        assets = work / "assets"
        binary.mkdir()
        state.mkdir()
        assets.mkdir()
        first_asset = assets / "service.deb"
        second_asset = assets / "service.rpm"
        amd64_binary = work / "binaries/amd64/service"
        arm64_binary = work / "binaries/arm64/service"
        first_asset.write_text("deb\n")
        second_asset.write_text("rpm\n")
        amd64_binary.parent.mkdir(parents=True)
        arm64_binary.parent.mkdir(parents=True)
        amd64_binary.write_text("amd64 binary\n")
        arm64_binary.write_text("arm64 binary\n")

        executable(
            binary / "gh",
            f"""import json
import os
import shutil
import sys
from pathlib import Path
arguments = sys.argv[1:]
if os.environ.get("GPG_PRIVATE_KEY") or os.environ.get("S3_SECRET_ACCESS_KEY"):
    raise SystemExit("release secrets leaked to gh")
if any(name.startswith("GITHUB_") for name in os.environ):
    raise SystemExit("GitHub runner environment leaked to gh")
log = Path({str(log)!r})
state = Path({str(state)!r})
with log.open("a") as stream:
    stream.write(json.dumps(["gh", *arguments]) + "\\n")
if arguments[:2] == ["release", "view"]:
    tag = arguments[2]
    if not (state / f"release-{{tag}}").exists():
        raise SystemExit(1)
    print(json.dumps({{"assets": [{{"name": "old one.deb"}}, {{"name": "old.rpm"}}]}}))
elif arguments[:2] == ["release", "create"]:
    tag = arguments[2]
    (state / f"release-{{tag}}").touch()
    (state / f"ref-{{tag}}").touch()
elif arguments[:2] == ["release", "upload"]:
    uploaded = state / "uploaded"
    uploaded.mkdir(exist_ok=True)
    for value in arguments[3:arguments.index("--repo")]:
        source = Path(value)
        shutil.copyfile(source, uploaded / source.name)
elif arguments[:2] == ["api", "repos/example/service/git/ref/tags/nightly"]:
    raise SystemExit(0 if (state / "ref-nightly").exists() else 1)
""",
        )
        executable(
            binary / "docker",
            f"""import json
import os
import sys
from pathlib import Path
if os.environ.get("GPG_PRIVATE_KEY") or os.environ.get("S3_SECRET_ACCESS_KEY"):
    raise SystemExit("release secrets leaked to docker")
with Path({str(log)!r}).open("a") as stream:
    stream.write(json.dumps(["docker", *sys.argv[1:]]) + "\\n")
""",
        )

        environment = dict(os.environ)
        environment.update(
            PATH=f"{binary}:{environment['PATH']}",
            GH_TOKEN="github-token",
            GPG_PRIVATE_KEY="must-not-leak",
            S3_SECRET_ACCESS_KEY="must-not-leak",
            GITHUB_REPOSITORY="must-not-leak",
        )
        command = run.set(env=environment, inherit_env=False)
        revision = "0123456789abcdef0123456789abcdef01234567"

        await command(
            sys.executable,
            task,
            "--repository",
            "example/service",
            "--channel",
            "nightly",
            "--tag",
            "nightly",
            "--title",
            "nightly",
            "--revision",
            revision,
            "--version",
            "1.2.3-nightly.1700000000",
            "--checks-url",
            "https://example.test/actions/runs/42",
            "--asset",
            first_asset,
            second_asset,
            "--binary",
            f"service-linux-amd64={amd64_binary}",
            f"service-linux-arm64={arm64_binary}",
            "--image",
            "ghcr.io/example/service:1.2.3-nightly.1700000000",
            "--image-alias",
            "ghcr.io/example/service:nightly",
        )
        commands = entries(log)
        create = next(
            entry for entry in commands if entry[1:3] == ["release", "create"]
        )
        assert "--prerelease" in create
        assert create[create.index("--target") + 1] == revision
        assert "Release checks" in create[create.index("--notes") + 1]
        upload_index = next(
            index
            for index, entry in enumerate(commands)
            if entry[1:3] == ["release", "upload"]
        )
        docker_index = next(
            index for index, entry in enumerate(commands) if entry[0] == "docker"
        )
        patch_index = next(
            index
            for index, entry in enumerate(commands)
            if entry[1:4] == ["api", "--method", "PATCH"]
        )
        assert upload_index < docker_index < patch_index
        uploaded = state / "uploaded"
        assert sorted(path.name for path in uploaded.iterdir()) == [
            "SHA256SUMS",
            "service-linux-amd64",
            "service-linux-arm64",
            "service.deb",
            "service.rpm",
        ]
        expected_checksums = "".join(
            f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {name}\n"
            for name, path in (
                ("service-linux-amd64", amd64_binary),
                ("service-linux-arm64", arm64_binary),
            )
        )
        assert (uploaded / "SHA256SUMS").read_text() == expected_checksums

        log.write_text("")
        shutil.rmtree(uploaded)
        (state / "release-v1.2.3").touch()
        await command(
            sys.executable,
            task,
            "--repository",
            "example/service",
            "--channel",
            "stable",
            "--tag",
            "v1.2.3",
            "--title",
            "1.2.3",
            "--revision",
            revision,
            "--version",
            "1.2.3",
            "--asset",
            first_asset,
            second_asset,
            "--binary",
            f"service-linux-amd64={amd64_binary}",
            f"service-linux-arm64={arm64_binary}",
        )
        commands = entries(log)
        deleted = [
            entry[4] for entry in commands if entry[1:3] == ["release", "delete-asset"]
        ]
        assert deleted == ["old one.deb", "old.rpm"]
        edit = next(entry for entry in commands if entry[1:3] == ["release", "edit"])
        assert "--prerelease" not in edit
        assert not any(entry[1:2] == ["api"] for entry in commands)
        assert sorted(path.name for path in uploaded.iterdir()) == [
            "SHA256SUMS",
            "service-linux-amd64",
            "service-linux-arm64",
            "service.deb",
            "service.rpm",
        ]
        assert (uploaded / "SHA256SUMS").read_text() == expected_checksums

        missing = await command.result(
            sys.executable,
            task,
            "--repository",
            "example/service",
            "--channel",
            "stable",
            "--tag",
            "v1.2.3",
            "--title",
            "1.2.3",
            "--revision",
            revision,
            "--version",
            "1.2.3",
            "--asset",
            work / "missing",
        ).stderr(sh.DEVNULL)
        assert missing.exit_code != 0

        missing_binary = await command.result(
            sys.executable,
            task,
            "--repository",
            "example/service",
            "--channel",
            "nightly",
            "--tag",
            "nightly",
            "--title",
            "nightly",
            "--revision",
            revision,
            "--version",
            "1.2.3-nightly.1700000000",
            "--asset",
            first_asset,
            "--binary",
            f"service-linux-amd64={work / 'missing'}",
        ).stderr(sh.DEVNULL)
        assert missing_binary.exit_code != 0

        duplicate = await command.result(
            sys.executable,
            task,
            "--repository",
            "example/service",
            "--channel",
            "nightly",
            "--tag",
            "nightly",
            "--title",
            "nightly",
            "--revision",
            revision,
            "--version",
            "1.2.3-nightly.1700000000",
            "--asset",
            first_asset,
            "--binary",
            f"service.deb={amd64_binary}",
        ).stderr(sh.DEVNULL)
        assert duplicate.exit_code != 0

    print("github release: ok")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
