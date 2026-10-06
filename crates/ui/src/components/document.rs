// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use maud::{DOCTYPE, Markup, Render, html};

use crate::{ASSET_PREFIX, Asset, AssetLookup, AssetsCatalog, FAVICON_ICO_PATH};

pub struct Document<'a> {
    title: &'a str,
    body: Markup,
    head: Option<Markup>,
    catalog: &'a AssetsCatalog,
    assets: u8,
}

const MANIFEST: u8 = 1 << 1;
const SVG_ICON: u8 = 1 << 2;
const APPLE_TOUCH_ICON: u8 = 1 << 3;

impl<'a> Document<'a> {
    #[must_use]
    pub fn new(title: &'a str, body: Markup, catalog: &'a AssetsCatalog) -> Self {
        Self {
            title,
            body,
            head: None,
            catalog,
            assets: 0,
        }
    }

    #[must_use]
    pub fn with_head(mut self, head: Markup) -> Self {
        self.head = Some(head);
        self
    }

    #[must_use]
    pub const fn with_manifest(mut self) -> Self {
        self.assets |= MANIFEST;
        self
    }

    #[must_use]
    pub const fn with_svg_icon(mut self) -> Self {
        self.assets |= SVG_ICON;
        self
    }

    #[must_use]
    pub const fn with_apple_touch_icon(mut self) -> Self {
        self.assets |= APPLE_TOUCH_ICON;
        self
    }
}

impl Render for Document<'_> {
    fn render(&self) -> Markup {
        let favicon = self.catalog.lookup("favicon.ico").map(AssetLookup::asset);
        let svg_icon = (self.assets & SVG_ICON != 0)
            .then(|| self.catalog.lookup("favicon.svg"))
            .flatten()
            .map(AssetLookup::asset);
        let apple_touch_icon = (self.assets & APPLE_TOUCH_ICON != 0)
            .then(|| self.catalog.lookup("apple-touch-icon.png"))
            .flatten()
            .map(AssetLookup::asset);
        let manifest = (self.assets & MANIFEST != 0)
            .then(|| self.catalog.lookup("manifest.webmanifest"))
            .flatten()
            .map(AssetLookup::asset);

        html! {
            (DOCTYPE)
            html lang="en" {
                head {
                    meta charset="UTF-8";
                    meta name="viewport" content="width=device-width,initial-scale=1";

                    title { (self.title) }

                    @if favicon.is_some() {
                        link rel="icon" href=(FAVICON_ICO_PATH);
                    }
                    @if let Some(asset) = svg_icon {
                        link rel="icon" href=(asset_path(asset)) type="image/svg+xml" sizes="any";
                    }
                    @if let Some(asset) = apple_touch_icon {
                        link rel="apple-touch-icon" href=(asset_path(asset));
                    }
                    @if let Some(asset) = manifest {
                        link rel="manifest" href=(asset_path(asset));
                    }

                    @for asset in self.catalog.stylesheets() {
                        link rel="stylesheet" href=(asset_path(asset)) integrity=(asset.integrity());
                    }
                    @for asset in self.catalog.scripts() {
                        script src=(asset_path(asset)) integrity=(asset.integrity()) defer {}
                    }

                    @if let Some(head) = &self.head {
                        (head)
                    }
                }
                body { (&self.body) }
            }
        }
    }
}

fn asset_path(asset: Asset) -> String {
    format!("{ASSET_PREFIX}/{}", asset.fingerprinted_name())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AssetKind;

    const INTEGRITY: &str =
        "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS";
    const ASSETS: &[Asset] = &[
        Asset::new(
            AssetKind::Stylesheet,
            "global.css",
            "global.0000000000000001.css",
            crate::CachePolicy::Immutable,
            b"global",
            INTEGRITY,
        ),
        Asset::new(
            AssetKind::Stylesheet,
            "app.css",
            "app.0000000000000002.css",
            crate::CachePolicy::Immutable,
            b"app",
            INTEGRITY,
        ),
        Asset::new(
            AssetKind::Script,
            "app.js",
            "app.0000000000000003.js",
            crate::CachePolicy::Immutable,
            b"script",
            INTEGRITY,
        ),
        Asset::new(
            AssetKind::Static {
                content_type: "image/x-icon",
            },
            "favicon.ico",
            "favicon.0000000000000004.ico",
            crate::CachePolicy::Immutable,
            b"icon",
            INTEGRITY,
        ),
        Asset::new(
            AssetKind::Static {
                content_type: "image/svg+xml",
            },
            "favicon.svg",
            "favicon.0000000000000005.svg",
            crate::CachePolicy::Immutable,
            b"svg",
            INTEGRITY,
        ),
        Asset::new(
            AssetKind::Static {
                content_type: "image/png",
            },
            "apple-touch-icon.png",
            "apple-touch-icon.0000000000000006.png",
            crate::CachePolicy::Immutable,
            b"png",
            INTEGRITY,
        ),
        Asset::new(
            AssetKind::Static {
                content_type: "application/manifest+json",
            },
            "manifest.webmanifest",
            "manifest.0000000000000007.webmanifest",
            crate::CachePolicy::Immutable,
            b"manifest",
            INTEGRITY,
        ),
    ];

    #[test]
    fn renders_ordered_catalog_assets_without_inline_code() {
        let catalog = AssetsCatalog::new().with(ASSETS).unwrap();
        let rendered = Document::new("<Service>", html! { main { "ok" } }, &catalog)
            .with_manifest()
            .with_svg_icon()
            .with_apple_touch_icon()
            .with_head(html! { meta name="generator" content="test"; })
            .render()
            .into_string();

        assert!(rendered.starts_with("<!DOCTYPE html>"));
        assert!(rendered.contains("&lt;Service&gt;"));
        assert!(rendered.contains("global.0000000000000001.css"));
        assert!(rendered.contains("app.0000000000000002.css"));
        assert!(rendered.contains("app.0000000000000003.js"));
        assert!(rendered.contains(&format!("integrity=\"{INTEGRITY}\"")));
        assert!(rendered.contains(FAVICON_ICO_PATH));
        assert!(rendered.contains("favicon.0000000000000005.svg"));
        assert!(rendered.contains("apple-touch-icon.0000000000000006.png"));
        assert!(rendered.contains("manifest.0000000000000007.webmanifest"));
        assert!(rendered.contains("name=\"generator\""));
        assert!(!rendered.contains("<style"));
        assert!(
            rendered.find("global.0000000000000001.css")
                < rendered.find("app.0000000000000002.css")
        );
    }

    #[test]
    fn omits_script_element_when_catalog_has_no_script() {
        let catalog = AssetsCatalog::new().with(&ASSETS[..2]).unwrap();
        let rendered = Document::new("Service", html! { main { "ok" } }, &catalog)
            .render()
            .into_string();
        assert!(!rendered.contains("<script"));
    }
}
