# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0

from __future__ import annotations

from .common import TaskError, capture, require_command, run

TASK = "publish"


async def publish(context) -> None:
    require_command("apt-ftparchive", TASK)
    require_command("dpkg-deb", TASK)
    root = context.work / "apt"
    pool = root / "pool" / context.channel
    metadata = root / "dists" / context.channel
    pool.mkdir(parents=True)
    pool_prefix = context.storage.service_key("apt", "pool", context.channel)
    metadata_prefix = context.storage.service_key("apt", "dists", context.channel)
    context.storage.download_prefix(pool_prefix, pool, "*.deb")
    for package in context.packages["deb"]:
        context.add_package(package, pool)

    architectures = sorted(
        {
            (await capture("dpkg-deb", "-f", package, "Architecture")).strip()
            for package in pool.glob("*.deb")
        }
    )
    if not architectures or "" in architectures:
        raise TaskError(f"{TASK}: no DEB architectures found")
    for architecture in architectures:
        (metadata / "main" / f"binary-{architecture}").mkdir(
            parents=True, exist_ok=True
        )

    cache = context.work / "apt-cache"
    cache.mkdir()
    config = context.work / "apt-ftparchive.conf"
    architecture_list = " ".join(architectures)
    config.write_text(
        f'''Dir {{ ArchiveDir "{root}"; CacheDir "{cache}"; }};
Default {{ Packages::Compress ". gzip"; Packages::Extensions ".deb"; }};
TreeDefault {{
    Packages "$(DIST)/$(SECTION)/binary-$(ARCH)/Packages";
    BinCacheDB "packages-$(ARCH).db";
}};
Tree "dists/{context.channel}" {{
    Sections "main";
    Architectures "{architecture_list}";
    Directory "pool/{context.channel}";
}};
'''
    )
    await run("apt-ftparchive", "generate", config)
    release = metadata / "Release"
    await run(
        "apt-ftparchive",
        "-o",
        "APT::FTPArchive::Release::Origin=Dimidium Labs",
        "-o",
        f"APT::FTPArchive::Release::Label={context.service} {context.channel}",
        "-o",
        f"APT::FTPArchive::Release::Suite={context.channel}",
        "-o",
        f"APT::FTPArchive::Release::Codename={context.channel}",
        "-o",
        "APT::FTPArchive::Release::Components=main",
        "-o",
        f"APT::FTPArchive::Release::Architectures={architecture_list}",
        "release",
        f"{metadata}/",
    ).stdout(release)
    await context.gpg.sign(
        metadata / "Release.gpg", "--armor", "--detach-sign", release
    )
    await context.gpg.sign(metadata / "InRelease", "--clearsign", release)
    context.storage.upload_payloads(pool, pool_prefix, "*.deb")
    context.storage.replace_prefix(metadata, metadata_prefix)
