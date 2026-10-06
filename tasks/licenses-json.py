#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# fmt: off
#MISE description="Generate embeddable Rust dependency license JSON"
#MISE tools={"pipx"="1.16.7","python"="3.14.7","github:EmbarkStudios/cargo-about"="0.8.4"}
# fmt: on
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import argparse
import json
import re
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path
from typing import Any

sys.dont_write_bytecode = True

from libs.common import TaskError, capture, require_command, run, task_main

TASK = "licenses-json"
# 0.8.4 intentionally matches the license-file deduplication behavior of the
# legacy in-tree generator. Keep the task tool pin and normalizer in sync.
SPDX_TOKEN = re.compile(r"[A-Za-z0-9][A-Za-z0-9.+:-]*|[()]")
SPDX_OPERATORS = {"AND", "OR", "WITH"}


def license_requirements(expression: str) -> list[str]:
    """Return SPDX requirements in expression order, matching the legacy generator."""
    normalized = expression.replace("/", " OR ")
    tokens = SPDX_TOKEN.findall(normalized)
    requirements: list[str] = []

    index = 0
    while index < len(tokens):
        token = tokens[index]
        if token in {"(", ")", "AND", "OR"}:
            index += 1
            continue
        if token == "WITH":
            raise TaskError(f"{TASK}: invalid SPDX expression: {expression}")

        requirement = token
        if index + 1 < len(tokens) and tokens[index + 1] == "WITH":
            if index + 2 >= len(tokens) or tokens[index + 2] in SPDX_OPERATORS | {
                "(",
                ")",
            }:
                raise TaskError(f"{TASK}: invalid SPDX expression: {expression}")
            requirement = f"{token} WITH {tokens[index + 2]}"
            index += 2

        if requirement not in requirements:
            requirements.append(requirement)
        index += 1

    return requirements


def cargo_about_config(metadata: dict[str, Any]) -> str:
    accepted_by_crate: dict[str, list[str]] = {}
    for package in metadata.get("packages", []):
        expression = package.get("license")
        if not expression:
            continue

        accepted = accepted_by_crate.setdefault(package["name"], [])
        for requirement in license_requirements(expression):
            if requirement not in accepted:
                accepted.append(requirement)

    lines = [
        "accepted = []",
        "private = { ignore = true }",
        "ignore-build-dependencies = true",
        "ignore-dev-dependencies = true",
        "ignore-transitive-dependencies = false",
    ]
    for name, accepted in sorted(accepted_by_crate.items()):
        lines.extend(
            ("", f"[{json.dumps(name)}]", f"accepted = {json.dumps(accepted)}")
        )

    return "\n".join(lines) + "\n"


def bundled_license_sources(metadata: dict[str, Any]) -> list[dict[str, Any]]:
    """Load explicitly declared licenses for non-Rust resources bundled by crates."""
    sources: list[dict[str, Any]] = []
    for package in sorted(metadata.get("packages", []), key=lambda item: item["name"]):
        package_metadata = package.get("metadata") or {}
        dimidiumlabs = package_metadata.get("dimidiumlabs", {})
        if not isinstance(dimidiumlabs, dict):
            raise TaskError(
                f"{TASK}: package {package['name']}: "
                "metadata.dimidiumlabs must be a table"
            )
        bundled = dimidiumlabs.get("bundled-licenses", [])
        if not isinstance(bundled, list):
            raise TaskError(
                f"{TASK}: package {package['name']}: "
                "metadata.dimidiumlabs.bundled-licenses must be an array"
            )

        manifest_directory = Path(package["manifest_path"]).parent
        for declaration in bundled:
            if not isinstance(declaration, dict):
                raise TaskError(
                    f"{TASK}: package {package['name']}: "
                    "bundled license declaration must be a table"
                )
            try:
                identifier = declaration["id"]
                name = declaration["name"]
                relative_path = declaration["path"]
            except KeyError as error:
                raise TaskError(
                    f"{TASK}: package {package['name']}: bundled license is missing "
                    f"{error.args[0]}"
                ) from error
            if not all(
                isinstance(value, str) and value
                for value in (identifier, name, relative_path)
            ):
                raise TaskError(
                    f"{TASK}: package {package['name']}: bundled license fields "
                    "id, name, and path must be non-empty strings"
                )
            path = Path(relative_path)
            if path.is_absolute():
                raise TaskError(
                    f"{TASK}: package {package['name']}: bundled license path "
                    "must be relative to Cargo.toml"
                )
            try:
                text = (manifest_directory / path).read_text(encoding="utf-8")
            except OSError as error:
                raise TaskError(
                    f"{TASK}: package {package['name']}: cannot read bundled "
                    f"license {relative_path}: {error}"
                ) from error
            sources.append(
                {
                    "name": name,
                    "id": identifier,
                    "text": text,
                    "used_by": [
                        {
                            "crate": {
                                "name": package["name"],
                                "version": package["version"],
                                "repository": package.get("repository"),
                            }
                        }
                    ],
                }
            )
    return sources


def merge_bundled_licenses(
    report: dict[str, Any], bundled: Sequence[dict[str, Any]]
) -> None:
    licenses = report.setdefault("licenses", [])
    for source in bundled:
        existing = next(
            (
                candidate
                for candidate in licenses
                if candidate["id"] == source["id"]
                and candidate["text"] == source["text"]
            ),
            None,
        )
        if existing is None:
            licenses.append(source)
            continue
        known = {
            (usage["crate"]["name"], usage["crate"]["version"])
            for usage in existing["used_by"]
        }
        for usage in source["used_by"]:
            identity = (usage["crate"]["name"], usage["crate"]["version"])
            if identity not in known:
                existing["used_by"].append(usage)
                known.add(identity)


def normalized_output(report: dict[str, Any]) -> str:
    licenses: list[dict[str, Any]] = []
    for source in report.get("licenses", []):
        used_by = [
            {
                "crate": {
                    "name": usage["crate"]["name"],
                    "version": usage["crate"]["version"],
                    "repository": usage["crate"].get("repository"),
                }
            }
            for usage in source["used_by"]
        ]
        used_by.sort(key=lambda usage: len(usage["crate"]["name"]))
        licenses.append(
            {
                "name": source["name"],
                "id": source["id"],
                "first_of_kind": False,
                "text": source["text"],
                "used_by": used_by,
            }
        )

    licenses.sort(key=lambda license_: license_["id"])

    overview_by_id: dict[str, dict[str, Any]] = {}
    for license_ in licenses:
        first = license_["id"] not in overview_by_id
        license_["first_of_kind"] = first
        overview = overview_by_id.setdefault(
            license_["id"],
            {
                "count": 0,
                "name": license_["name"],
                "id": license_["id"],
            },
        )
        overview["count"] += len(license_["used_by"])

    overview = sorted(overview_by_id.values(), key=lambda item: item["name"])
    output = {"overview": overview, "licenses": licenses}
    return json.dumps(output, ensure_ascii=False, indent=2) + "\n"


async def cargo_metadata(manifest_path: Path, offline: bool) -> dict[str, Any]:
    arguments: list[str | Path] = [
        "cargo",
        "metadata",
        "--format-version",
        "1",
        "--locked",
        "--manifest-path",
        manifest_path,
    ]
    if offline:
        arguments.append("--offline")

    output = await capture(arguments)
    try:
        return json.loads(output)
    except json.JSONDecodeError as error:
        raise TaskError(
            f"{TASK}: cargo metadata returned invalid JSON: {error}"
        ) from error


async def generate(
    manifest_path: Path,
    targets: Sequence[str],
    offline: bool,
) -> str:
    metadata = await cargo_metadata(manifest_path, offline)
    bundled = bundled_license_sources(metadata)

    with tempfile.TemporaryDirectory(prefix=f"{TASK}-") as temporary:
        work = Path(temporary)
        config = work / "about.toml"
        report = work / "report.json"
        config.write_text(cargo_about_config(metadata), encoding="utf-8")

        arguments: list[str | Path] = [
            "cargo-about",
            "generate",
            "--config",
            config,
            "--manifest-path",
            manifest_path,
            "--format",
            "json",
            "--locked",
            "--output-file",
            report,
        ]
        if offline:
            arguments.append("--offline")
        for target in targets:
            arguments.extend(("--target", target))

        quiet_arguments = ["cargo-about", "-L", "off", *arguments[1:]]
        result = await capture.result(quiet_arguments)
        if result.exit_code != 0:
            # Repeat with diagnostics enabled only on failure. cargo-about 0.8.4
            # otherwise reports harmless scanner errors for deprecated SPDX IDs.
            await run(arguments)
        try:
            raw_report = json.loads(report.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as error:
            raise TaskError(
                f"{TASK}: cargo-about returned invalid JSON: {error}"
            ) from error

    merge_bundled_licenses(raw_report, bundled)
    return normalized_output(raw_report)


async def main(args: Sequence[str]) -> None:
    command = argparse.ArgumentParser(prog="mise run licenses-json --")
    command.add_argument(
        "--manifest-path",
        default=Path("Cargo.toml"),
        type=Path,
    )
    command.add_argument("--output", required=True, type=Path)
    command.add_argument("--target", action="append", required=True)
    command.add_argument("--offline", action="store_true")
    command.add_argument("--check", action="store_true")
    arguments = command.parse_args(args)

    require_command("cargo", TASK)
    require_command("cargo-about", TASK)
    if not arguments.manifest_path.is_file():
        raise TaskError(f"{TASK}: manifest not found: {arguments.manifest_path}")

    output = await generate(
        arguments.manifest_path.resolve(),
        arguments.target,
        arguments.offline,
    )
    if arguments.check:
        if not arguments.output.is_file():
            raise TaskError(f"{TASK}: output not found: {arguments.output}")
        if arguments.output.read_text(encoding="utf-8") != output:
            raise TaskError(
                f"{TASK}: {arguments.output} is out of date; regenerate it without --check"
            )
        return

    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    if (
        not arguments.output.is_file()
        or arguments.output.read_text(encoding="utf-8") != output
    ):
        arguments.output.write_text(output, encoding="utf-8")


if __name__ == "__main__":
    task_main(TASK, main, sys.argv[1:])
