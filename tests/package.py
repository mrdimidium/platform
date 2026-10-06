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
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path

from shellous import sh

run = sh.stdout(sh.INHERIT).stderr(sh.INHERIT)
capture = sh.stderr(sh.INHERIT)


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
        stage = work / "stage"
        output = work / "out"
        binary.mkdir()
        (stage / "sub").mkdir(parents=True)
        output.mkdir()
        rpm_config = work / "rpm-config-source"
        rpm_config.mkdir()
        (rpm_config / "macros").write_text("%fixture 1\n")
        (stage / "tool").write_text("payload\n")
        (stage / "sub/file").write_text("nested\n")
        outside = work / "outside"
        outside.write_text("outside\n")
        (stage / "outside-link").symlink_to(outside)
        log = work / "package.log"

        executable(
            binary / "nfpm",
            f"""import os
import sys
from pathlib import Path
secrets = (
    "GPG_PRIVATE_KEY", "APK_PRIVATE_KEY", "SIGNING_PRIVATE_KEY",
    "NFPM_PASSPHRASE", "NFPM_DEB_PASSPHRASE", "NFPM_RPM_PASSPHRASE",
)
if any(os.environ.get(name) for name in secrets):
    print("raw private key leaked to nFPM", file=sys.stderr)
    raise SystemExit(1)
if any(name.startswith("GITHUB_") for name in os.environ):
    print("GitHub environment leaked to nFPM", file=sys.stderr)
    raise SystemExit(1)
if os.environ.get("UNRELATED_SENTINEL"):
    print("unrelated environment leaked to nFPM", file=sys.stderr)
    raise SystemExit(1)
arguments = sys.argv[1:]
config = Path(arguments[arguments.index("--config") + 1])
packager = arguments[arguments.index("--packager") + 1]
target = Path(arguments[arguments.index("--target") + 1])
with Path({str(log)!r}).open("a") as stream:
    stream.write(f"ARCH={{os.environ['ARCH']}}\\n")
    stream.write(f"VERSION={{os.environ['VERSION']}}\\n")
    stream.write(f"GPG_KEY_ID={{os.environ.get('GPG_KEY_ID', '')}}\\n")
    stream.write(f"{{packager}}\\n")
    for line in config.read_text().splitlines():
        if line.startswith("key_name:"):
            stream.write(f"{{line}}\\n")
(target / f"test.{{packager}}").touch()
""",
        )
        executable(
            binary / "gpg",
            f"""import sys
from pathlib import Path
with Path({str(log)!r}).open("a") as stream:
    stream.write(f"gpg {{' '.join(sys.argv[1:])}}\\n")
arguments = sys.argv[1:]
if "-o" in arguments:
    Path(arguments[arguments.index("-o") + 1]).write_text("signature")
""",
        )
        for name in ("debsigs", "rpmsign"):
            executable(
                binary / name,
                f"""import os
import subprocess
import sys
from pathlib import Path
name = Path(sys.argv[0]).name
with Path({str(log)!r}).open("a") as stream:
    stream.write(f"{{name}}\\n")
    if name == "rpmsign":
        macros = Path(os.environ["RPM_CONFIGDIR"]) / "macros"
        stream.write(f"rpm-macros={{macros.read_text().strip()}}\\n")
subprocess.run(["gpg", name], check=True)
""",
            )
        executable(
            binary / "rpm",
            f"""import sys
if sys.argv[1:] != ["--eval", "%{{_rpmconfigdir}}"]:
    raise SystemExit(1)
print({str(rpm_config)!r})
""",
        )
        executable(
            binary / "openssl",
            """import sys
from pathlib import Path
arguments = sys.argv[1:]
Path(arguments[arguments.index("-out") + 1]).write_text("public key\\n")
""",
        )

        config = work / "nfpm.yaml"
        config.write_text("name: test\n")
        environment = dict(os.environ)
        environment.update(
            PATH=f"{binary}:{environment['PATH']}",
            GITHUB_REPOSITORY="provider/repository",
            GITHUB_TOKEN="provider-token",
            UNRELATED_SENTINEL="must-not-leak",
        )
        command = run.set(env=environment, inherit_env=False)
        package = root / "tasks/package.py"

        await command(
            package,
            "--output",
            output,
            "--archive-root",
            stage,
            "--archive-name",
            "test-linux-amd64",
            "tar.gz",
            "zip",
        )
        tarball = output / "test-linux-amd64.tar.gz"
        zipfile = output / "test-linux-amd64.zip"
        assert tarball.is_file()
        assert zipfile.is_file()
        assert "./tool" in await capture("tar", "-tzf", tarball)
        assert "sub/file" in await capture("unzip", "-l", zipfile)
        assert (await capture("unzip", "-p", zipfile, "outside-link")) == str(outside)

        await command(
            package,
            "--config",
            config,
            "--version",
            "1.2.3~nightly.42",
            "--arch",
            "arm64",
            "--output",
            output,
            "deb",
            "rpm",
        )
        assert (output / "test.deb").is_file()
        assert (output / "test.rpm").is_file()
        assert "ARCH=arm64" in log.read_text().splitlines()
        assert "VERSION=1.2.3~nightly.42" in log.read_text().splitlines()

        signing_environment = dict(environment)
        signing_environment.update(
            GPG_PRIVATE_KEY="private",
            GPG_PASSPHRASE="passphrase",
            GPG_KEY_ID="0123456789ABCDEF0123456789ABCDEF01234567",
            APK_PRIVATE_KEY="apk-private",
            SOURCE_DATE_EPOCH="1700000000",
            SIGNING_PRIVATE_KEY="raw-signing-private",
            NFPM_PASSPHRASE="raw-passphrase",
            NFPM_DEB_PASSPHRASE="raw-deb-passphrase",
            NFPM_RPM_PASSPHRASE="raw-rpm-passphrase",
        )
        await run.set(env=signing_environment, inherit_env=False)(
            package,
            "--config",
            config,
            "--version",
            "1.2.3",
            "--arch",
            "amd64",
            "--output",
            output,
            "--apk-public-key",
            "test.rsa.pub",
            "deb",
            "rpm",
            "apk",
        )
        assert (output / "test.apk").is_file()
        assert (output / "test.rsa.pub").is_file()
        log_lines = log.read_text().splitlines()
        for expected in (
            "debsigs",
            "rpmsign",
            "GPG_KEY_ID=89ABCDEF01234567",
        ):
            assert expected in log_lines
        gpg_lines = [line for line in log_lines if line.startswith("gpg ")]
        assert gpg_lines
        assert all("--faked-system-time 1700000000!" in line for line in gpg_lines), (
            gpg_lines
        )
        assert "%_gpg_sign_cmd_extra_args --faked-system-time 1700000000!" in log_lines

        invalid_timestamp_environment = dict(
            signing_environment, SOURCE_DATE_EPOCH="not-a-timestamp"
        )
        result = await run.result.set(
            env=invalid_timestamp_environment, inherit_env=False
        )(
            package,
            "--config",
            config,
            "--version",
            "1.2.3",
            "--arch",
            "amd64",
            "--output",
            output,
            "deb",
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0

        apk_environment = dict(environment, APK_PRIVATE_KEY="apk-private")
        await run.set(env=apk_environment, inherit_env=False)(
            package,
            "--config",
            config,
            "--version",
            "1.2.3",
            "--arch",
            "amd64",
            "--output",
            output,
            "apk",
        )

        versioned_config = work / "versioned.yaml"
        versioned_config.write_text("key_name: packages.${PACKAGE_KEY_VERSION}\n")
        result = await run.result.set(env=apk_environment, inherit_env=False)(
            package,
            "--config",
            versioned_config,
            "--version",
            "1.2.3",
            "--arch",
            "amd64",
            "--output",
            output,
            "apk",
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0

        versioned_environment = dict(apk_environment, PACKAGE_KEY_VERSION="0001")
        await run.set(env=versioned_environment, inherit_env=False)(
            package,
            "--config",
            versioned_config,
            "--version",
            "1.2.3",
            "--arch",
            "amd64",
            "--output",
            output,
            "apk",
        )
        assert "key_name: packages.0001" in log.read_text().splitlines()

        invalid_commands = (
            (package, "--output", output, "deb"),
            (
                package,
                "--output",
                output,
                "--archive-root",
                stage,
                "--archive-name",
                "../escape",
                "zip",
            ),
        )
        for arguments in invalid_commands:
            result = await command.result(*arguments).stderr(sh.DEVNULL)
            assert result.exit_code != 0

        external_key_environment = dict(
            environment, APK_SIGNING_KEY=str(work / "apk.rsa")
        )
        result = await run.result.set(env=external_key_environment, inherit_env=False)(
            package,
            "--config",
            config,
            "--version",
            "1.2.3",
            "--arch",
            "amd64",
            "--output",
            output,
            "--apk-public-key",
            "../escape",
            "apk",
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
