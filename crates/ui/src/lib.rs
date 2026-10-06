// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

extern crate self as dimidiumlabs_ui;

pub mod assets;
pub mod components;

pub use assets::{
    APPLE_TOUCH_ICON_PATH, ASSET_PREFIX, Asset, AssetKind, AssetLookup, AssetsCatalog,
    AssetsCatalogError, CachePolicy, EncodedAsset, FAVICON_ICO_PATH, FAVICON_SVG_PATH, FONT_PREFIX,
    MANIFEST_PATH, ROBOTS_PATH,
};
pub use components::Document;

mod generated_assets {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

pub const FOUNDATION: &[Asset] = generated_assets::FOUNDATION;

#[cfg(test)]
mod tests {
    #[test]
    fn global_styles_provide_semantic_typographic_rhythm() {
        let styles = include_str!("styles/global.css");

        for declaration in [
            "--font-family-roman: 'IBM Plex Sans', 'IBM Plex Math', sans-serif",
            "--font-family-mono: 'IBM Plex Mono', 'IBM Plex Math', monospace",
            "font-family: var(--font-family-roman)",
            "font-family: var(--font-family-mono)",
            "--readable-line-width: 32em",
            "font-size: 1.125rem",
            "line-height: 1.4",
            "@media (min-width: 40rem)",
            "margin-bottom: 1rem",
            "--list-item-spacing: 0.875rem",
        ] {
            assert!(styles.contains(declaration), "missing {declaration}");
        }

        for selector in [
            "h1,\nh2,\nh3,\nh4,\nh5,\nh6",
            ":is(ul, ol)",
            "blockquote",
            "figure",
            "figcaption",
            "pre code",
        ] {
            assert!(styles.contains(selector), "missing {selector}");
        }
    }
}
