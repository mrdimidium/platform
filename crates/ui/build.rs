// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

fn main() {
    dimidiumlabs_ui_build::build("FOUNDATION", &["src/styles"], &["src/assets"])
        .expect("failed to compile shared UI assets");
}
