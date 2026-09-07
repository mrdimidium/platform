#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: Apache-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import asyncio
import hashlib
import os
import re
import shutil
import sys
import tempfile
import time
import urllib.request
from collections.abc import Sequence
from pathlib import Path

from shellous import Result, sh

run = sh.stdout(sh.INHERIT).stderr(sh.INHERIT)
capture = sh.stderr(sh.INHERIT)
APK_TOOLS_URL = (
    "https://dl-cdn.alpinelinux.org/alpine/v3.22/main/x86_64/"
    "apk-tools-static-2.14.10-r0.apk"
)
APK_TOOLS_SHA256 = "c86e3822764e5fe19f41ce2e13553e48cac1ea4e74f858338e8d44bf0b616b61"


def executable(path: Path, source: str) -> None:
    path.write_text("#!/usr/bin/env python3\n" + source)
    path.chmod(0o755)


async def main(args: Sequence[str]) -> None:
    if args:
        raise SystemExit(f"unexpected arguments: {' '.join(args)}")
    root = Path(__file__).resolve().parent.parent
    with tempfile.TemporaryDirectory() as directory:
        work = Path(directory)
        remote = work / "remote/integration"
        binary = work / "bin"
        package_input = work / "input"
        fixture = work / "fixture"
        for path in (remote, binary, package_input, fixture):
            path.mkdir(parents=True)

        payload = fixture / "publish-contract"
        payload.write_text("publish integration fixture\n")
        config = fixture / "nfpm.yaml"
        config.write_text(
            f"""name: publish-contract
arch: ${{ARCH}}
version: ${{VERSION}}
platform: linux
maintainer: Dimidium Labs <me@govorov.online>
description: Shared publish task integration fixture
license: 0BSD
contents:
  - src: {payload}
    dst: /usr/local/bin/publish-contract
deb:
  signature:
    method: debsign
    key_id: ${{GPG_KEY_ID}}
    key_file: ${{SIGNING_PRIVATE_KEY}}
rpm:
  signature:
    key_id: ${{GPG_KEY_ID}}
    key_file: ${{SIGNING_PRIVATE_KEY}}
apk:
  signature:
    key_file: ${{APK_SIGNING_KEY}}
    key_name: packages.${{PACKAGE_KEY_VERSION}}
"""
        )

        source_gnupg = work / "source-gnupg"
        source_gnupg.mkdir(mode=0o700)
        source_environment = dict(os.environ, GNUPGHOME=str(source_gnupg))
        gpg = run.set(env=source_environment, inherit_env=False)
        gpg_capture = capture.set(env=source_environment, inherit_env=False)
        await gpg(
            "gpg",
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "integration-pass",
            "--quick-generate-key",
            "Publish Integration <publish@example.invalid>",
            "rsa2048",
            "sign",
            "1d",
        ).stdout(sh.DEVNULL)
        key_listing = await gpg_capture(
            "gpg", "--batch", "--with-colons", "--list-secret-keys"
        )
        fingerprint = next(
            fields[9]
            for line in key_listing.splitlines()
            if (fields := line.split(":"))[0] == "fpr"
        )
        private_gpg = work / "private.gpg"
        await gpg(
            "gpg",
            "--batch",
            "--pinentry-mode",
            "loopback",
            "--passphrase",
            "integration-pass",
            "--armor",
            "--export-secret-keys",
            fingerprint,
        ).stdout(private_gpg)
        private_rsa = work / "private.rsa"
        await (
            run("openssl", "genrsa", "-out", private_rsa, "2048")
            .stdout(sh.DEVNULL)
            .stderr(sh.DEVNULL)
        )

        key_version = "0001"
        keys = remote / "keys"
        keys.mkdir()
        public_gpg = remote / "packages.gpg"
        await gpg("gpg", "--batch", "--armor", "--export", fingerprint).stdout(
            public_gpg
        )
        shutil.copy2(public_gpg, keys / f"packages.{key_version}.gpg")
        await (
            run(
                "openssl",
                "rsa",
                "-in",
                private_rsa,
                "-pubout",
                "-out",
                keys / f"packages.{key_version}.rsa.pub",
            )
            .stdout(sh.DEVNULL)
            .stderr(sh.DEVNULL)
        )
        gpg_private_key = private_gpg.read_text()
        apk_private_key = private_rsa.read_text()
        source_date_epoch = str(int(time.time()))
        for name in ("gpg", "openssl"):
            command = shutil.which(name)
            assert command is not None
            executable(
                binary / name,
                f"""import os
import sys
if any(name.startswith("GITHUB_") for name in os.environ):
    raise SystemExit("GitHub environment leaked to signer")
if os.environ.get("S3_SECRET_ACCESS_KEY"):
    raise SystemExit("S3 credentials leaked to signer")
if os.environ.get("UNRELATED_SENTINEL"):
    raise SystemExit("unrelated environment leaked to signer")
os.execv({command!r}, [{command!r}, *sys.argv[1:]])
""",
            )

        async def package_version(version: str) -> None:
            environment = dict(os.environ)
            environment.update(
                GPG_PRIVATE_KEY=gpg_private_key,
                GPG_PASSPHRASE="integration-pass",
                GPG_KEY_ID=fingerprint,
                APK_PRIVATE_KEY=apk_private_key,
                PACKAGE_KEY_VERSION=key_version,
                SOURCE_DATE_EPOCH=source_date_epoch,
            )
            await run.set(env=environment, inherit_env=False)(
                "mise",
                "--cd",
                root,
                "run",
                "package",
                "--",
                "--config",
                config,
                "--version",
                version,
                "--arch",
                "amd64",
                "--output",
                package_input,
                "deb",
                "rpm",
                "apk",
            )

        publish_environment = dict(os.environ)
        publish_environment.update(
            PUBLISH_REMOTE=str(work / "remote"),
            PUBLISH_TEST_LOG=str(work / "s3.log"),
            PUBLISH_TEST_PYTHONPATH=str(root / "tests/fakes"),
            PATH=f"{binary}:{os.environ['PATH']}",
            S3_BUCKET="integration",
            S3_ENDPOINT="https://example.invalid",
            S3_PUBLIC_URL="https://pkg.dimidiumlabs.io",
            S3_ACCESS_KEY_ID="integration",
            S3_SECRET_ACCESS_KEY="integration",
            GPG_PRIVATE_KEY=gpg_private_key,
            GPG_PASSPHRASE="integration-pass",
            GPG_KEY_ID=fingerprint,
            APK_PRIVATE_KEY=apk_private_key,
            PACKAGE_KEY_VERSION=key_version,
            GITHUB_REPOSITORY="provider/repository",
            UNRELATED_SENTINEL="must-not-leak",
        )
        publish_command = run.result.set(env=publish_environment, inherit_env=False)

        async def publish(*, quiet: bool = False) -> Result:
            command = publish_command(
                root / "tasks/publish.py",
                "--service",
                "publish-contract",
                "--channel",
                "nightly",
                "--input",
                package_input,
                "deb",
                "rpm",
                "apk",
            )
            if quiet:
                command = command.stderr(sh.DEVNULL)
            return await command

        await package_version("1.2.3~nightly.42")
        assert (await publish()).exit_code == 0

        apt_root = remote / "publish-contract/apt"
        rpm_root = remote / "publish-contract/rpm/nightly"
        apk_root = remote / "publish-contract/apk/nightly/x86_64"
        assert public_gpg.is_file()
        assert (keys / f"packages.{key_version}.gpg").is_file()
        assert (keys / f"packages.{key_version}.rsa.pub").is_file()
        assert len(list((apt_root / "pool/nightly").glob("*.deb"))) == 1
        assert len(list(rpm_root.glob("*.rpm"))) == 1
        assert len(list(apk_root.glob("*.apk"))) == 1
        packages_file = apt_root / "dists/nightly/main/binary-amd64/Packages"
        assert "Package: publish-contract" in packages_file.read_text().splitlines()
        repository_file = rpm_root / "publish-contract-nightly.repo"
        assert (
            "baseurl=https://pkg.dimidiumlabs.io/publish-contract/rpm/nightly/"
            in repository_file.read_text().splitlines()
        )

        keyring = work / "packages.gpg"
        await run("gpg", "--batch", "--dearmor", "-o", keyring, public_gpg)
        await run(
            "gpgv",
            "--keyring",
            keyring,
            apt_root / "dists/nightly/InRelease",
        ).stdout(sh.DEVNULL)
        await run(
            "gpgv",
            "--keyring",
            keyring,
            rpm_root / "repodata/repomd.xml.asc",
            rpm_root / "repodata/repomd.xml",
        ).stdout(sh.DEVNULL)

        apk_tool = next(
            (
                path
                for base in (
                    Path.home() / ".cache/mise",
                    Path.home() / ".local/share/mise",
                )
                if base.is_dir()
                for path in base.rglob("apk.static")
                if path.is_file()
            ),
            None,
        )
        if apk_tool is None:
            archive = work / "apk-tools-static.apk"
            await asyncio.to_thread(urllib.request.urlretrieve, APK_TOOLS_URL, archive)
            assert hashlib.sha256(archive.read_bytes()).hexdigest() == APK_TOOLS_SHA256
            apk_directory = work / "apk-tools"
            apk_directory.mkdir()
            await run(
                "tar",
                "-xzf",
                archive,
                "-C",
                apk_directory,
                "sbin/apk.static",
            ).stderr(sh.DEVNULL)
            apk_tool = apk_directory / "sbin/apk.static"
        apk_keys = work / "apk-keys"
        apk_keys.mkdir()
        shutil.copy2(keys / f"packages.{key_version}.rsa.pub", apk_keys)
        await run(
            apk_tool,
            "verify",
            "--keys-dir",
            apk_keys,
            apk_root / "APKINDEX.tar.gz",
        ).stdout(sh.DEVNULL)

        (apt_root / "dists/nightly/stale").write_text("stale")
        (rpm_root / "repodata/stale").write_text("stale")
        shutil.rmtree(package_input)
        package_input.mkdir()
        await package_version("1.2.3~nightly.43")
        assert (await publish()).exit_code == 0
        assert len(list((apt_root / "pool/nightly").glob("*.deb"))) == 2
        assert len(list(rpm_root.glob("*.rpm"))) == 2
        assert len(list(apk_root.glob("*.apk"))) == 2
        assert not (apt_root / "dists/nightly/stale").exists()
        assert not (rpm_root / "repodata/stale").exists()
        assert (
            packages_file.read_text().splitlines().count("Package: publish-contract")
            == 2
        )
        await run(
            apk_tool,
            "verify",
            "--keys-dir",
            apk_keys,
            apk_root / "APKINDEX.tar.gz",
        ).stdout(sh.DEVNULL)

        deb = next(package_input.glob("*.deb"))
        with deb.open("a") as stream:
            stream.write("\nchanged\n")
        assert (await publish(quiet=True)).exit_code != 0
        shutil.copy2(apt_root / "pool/nightly" / deb.name, deb)
        lock = remote / "publish-contract/_locks/nightly"
        lock.parent.mkdir(parents=True, exist_ok=True)
        lock.write_text('{"expires":9999999999}\n')
        assert (await publish(quiet=True)).exit_code != 0
        lock.unlink()
        (keys / f"packages.{key_version}.gpg").write_text("different key\n")
        assert (await publish(quiet=True)).exit_code != 0

        allowed_delete = re.compile(
            r"^publish-contract/(apt/dists|rpm/nightly/repodata|_locks/nightly)"
        )
        for line in (work / "s3.log").read_text().splitlines():
            fields = line.split(maxsplit=2)
            if len(fields) == 3 and fields[:2] == ["s3", "delete"]:
                assert allowed_delete.match(fields[2])

        invalid_environment = dict(
            os.environ, PUBLISH_TEST_PYTHONPATH=str(root / "tests/fakes")
        )
        result = await run.result.set(env=invalid_environment, inherit_env=False)(
            root / "tasks/publish.py",
            "--service",
            "../escape",
            "--channel",
            "nightly",
            "--input",
            package_input,
            "deb",
        ).stderr(sh.DEVNULL)
        assert result.exit_code != 0


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1:]))
