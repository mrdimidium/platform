# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

import shutil

from .common import TaskError, capture, require_command, run

TASK = "publish"


async def publish(context) -> None:
    require_command("createrepo_c", TASK)
    require_command("rpmkeys", TASK)
    root = context.work / "rpm"
    root.mkdir()
    remote = context.storage.service_key("rpm", context.channel)
    context.storage.download_prefix(remote, root, "*.rpm")
    for package in context.packages["rpm"]:
        context.add_package(package, root)

    rpm_database = context.work / "rpmdb"
    rpm_database.mkdir()
    await run("rpmkeys", "--dbpath", rpm_database, "--import", context.gpg_public_key)
    for package in root.glob("*.rpm"):
        result = await capture(
            "rpmkeys", "--dbpath", rpm_database, "--checksig", package
        )
        if "signatures OK" not in result:
            raise TaskError(
                f"{TASK}: RPM is not signed by a trusted key: {package.name}"
            )

    shutil.rmtree(root / "repodata", ignore_errors=True)
    await run("createrepo_c", root)
    repomd = root / "repodata" / "repomd.xml"
    await context.gpg.sign(
        repomd.with_suffix(".xml.asc"), "--armor", "--detach-sign", repomd
    )
    definition = root / f"{context.service}-{context.channel}.repo"
    definition.write_text(
        f"""[{context.service}-{context.channel}]
name={context.service} {context.channel}
gpgkey={context.public_url}/packages.gpg
baseurl={context.public_url}/{context.service}/rpm/{context.channel}/
enabled=1
gpgcheck=1
repo_gpgcheck=1
"""
    )
    context.storage.upload_payloads(root, remote, "*.rpm")
    context.storage.upload(definition, f"{remote}/{definition.name}")
    context.storage.replace_prefix(root / "repodata", f"{remote}/repodata")
