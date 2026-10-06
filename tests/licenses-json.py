#!/usr/bin/env -S pipx run --backend pip
# SPDX-FileCopyrightText: 2026 Nikolay Govorov
# SPDX-License-Identifier: MPL-2.0
# /// script
# requires-python = ">=3.11"
# dependencies = ["shellous==0.42.0"]
# ///

from __future__ import annotations

import importlib.util
import json
import sys
from pathlib import Path
from types import ModuleType


def load_task() -> ModuleType:
    path = Path(__file__).parents[1] / "tasks/licenses-json.py"
    sys.path.insert(0, str(path.parent))
    spec = importlib.util.spec_from_file_location("licenses_json", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main() -> None:
    task = load_task()

    assert task.license_requirements("MIT/Apache-2.0") == ["MIT", "Apache-2.0"]
    assert task.license_requirements(
        "Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT"
    ) == ["Apache-2.0 WITH LLVM-exception", "Apache-2.0", "MIT"]

    config = task.cargo_about_config(
        {
            "packages": [
                {"name": "dual", "license": "MIT OR Apache-2.0"},
                {"name": "dual", "license": "MIT/Apache-2.0"},
                {"name": "private", "license": None},
            ]
        }
    )
    assert "accepted = []\nprivate = { ignore = true }" in config
    assert '["dual"]\naccepted = ["MIT", "Apache-2.0"]' in config
    assert '["private"]' not in config

    output = json.loads(
        task.normalized_output(
            {
                "overview": [],
                "licenses": [
                    {
                        "name": "MIT License",
                        "id": "MIT",
                        "first_of_kind": True,
                        "source_path": "/tmp/LICENSE",
                        "text": "MIT text",
                        "used_by": [
                            {
                                "crate": {
                                    "name": "long-name",
                                    "version": "2.0.0",
                                    "repository": None,
                                    "manifest_path": "/tmp/Cargo.toml",
                                },
                                "path": None,
                            },
                            {
                                "crate": {
                                    "name": "short",
                                    "version": "1.0.0",
                                    "repository": "https://example.invalid/short",
                                },
                                "path": None,
                            },
                        ],
                    },
                    {
                        "name": "Apache License 2.0",
                        "id": "Apache-2.0",
                        "first_of_kind": False,
                        "text": "Apache text",
                        "used_by": [
                            {
                                "crate": {
                                    "name": "dependency",
                                    "version": "3.0.0",
                                    "repository": None,
                                }
                            }
                        ],
                    },
                ],
                "crates": [{"package": {"manifest_path": "/tmp/Cargo.toml"}}],
            }
        )
    )

    assert output["overview"] == [
        {"count": 1, "name": "Apache License 2.0", "id": "Apache-2.0"},
        {"count": 2, "name": "MIT License", "id": "MIT"},
    ]
    assert [license_["id"] for license_ in output["licenses"]] == [
        "Apache-2.0",
        "MIT",
    ]
    assert all(license_["first_of_kind"] for license_ in output["licenses"])
    assert [usage["crate"]["name"] for usage in output["licenses"][1]["used_by"]] == [
        "short",
        "long-name",
    ]
    assert "source_path" not in output["licenses"][1]
    assert "manifest_path" not in output["licenses"][1]["used_by"][0]["crate"]
    assert "crates" not in output


if __name__ == "__main__":
    main()
