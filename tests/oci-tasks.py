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
        chart = project / "chart"
        (chart / "templates").mkdir(parents=True)
        (project / "Dockerfile").touch()
        (chart / "Chart.yaml").write_text(
            "apiVersion: v2\nname: fixture\nversion: 0.0.0\n"
        )
        (chart / "values.yaml").write_text("image: fixture\n")
        (chart / "templates/configmap.yaml").write_text(
            "apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: fixture\n"
        )
        binary.mkdir()
        executable(
            binary / "docker",
            """import os
import sys
from pathlib import Path
Path(os.environ["OCI_TEST_DOCKER_LOG"]).write_text("\\n".join(sys.argv[1:]) + "\\n")
""",
        )
        executable(
            binary / "helm",
            """import os
import sys
from pathlib import Path
arguments = sys.argv[1:]
log = Path(os.environ["OCI_TEST_HELM_LOG"])
with log.open("a") as stream:
    stream.write("\\n".join(arguments) + "\\n")
if arguments and arguments[0] == "package":
    destination = Path(arguments[arguments.index("--destination") + 1])
    version = arguments[arguments.index("--version") + 1]
    (destination / f"fixture-{version}.tgz").touch()
""",
        )

        docker_log = work / "docker.log"
        helm_log = work / "helm.log"
        environment = dict(os.environ)
        environment.update(
            PATH=f"{binary}:{environment['PATH']}",
            OCI_TEST_DOCKER_LOG=str(docker_log),
            OCI_TEST_HELM_LOG=str(helm_log),
        )
        command = run.set(env=environment, inherit_env=False)

        await command(
            root / "tasks/container.py",
            "--context",
            project,
            "--platform",
            "linux/amd64",
            "--target",
            "site",
            "--build-arg",
            "APP=site",
            "--build-arg",
            "TITLE=hello world",
            "--label",
            "org.example.title=Example site",
            "--tag",
            "ghcr.io/example/site:sha-abc",
            "--tag",
            "ghcr.io/example/site:latest",
            "--cache-from",
            "type=gha,scope=site",
            "--cache-to",
            "type=gha,mode=max,scope=site",
            "--provenance",
            "false",
            "--sbom",
            "false",
            "--push",
        )
        docker_arguments = docker_log.read_text().splitlines()
        for expected in (
            "buildx",
            "--target",
            "site",
            "TITLE=hello world",
            "org.example.title=Example site",
            "ghcr.io/example/site:sha-abc",
            "ghcr.io/example/site:latest",
            "type=gha,mode=max,scope=site",
            "--push",
        ):
            assert expected in docker_arguments

        await command(
            root / "tasks/chart.py",
            "--chart",
            chart,
            "--version",
            "1.2.3",
            "--app-version",
            "sha-abc",
            "--output",
            work / "output",
            "--push",
            "oci://ghcr.io/example/charts",
        )
        helm_arguments = helm_log.read_text().splitlines()
        for expected in (
            "lint",
            "package",
            "push",
            "oci://ghcr.io/example/charts",
        ):
            assert expected in helm_arguments

    print("oci tasks: ok")


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
