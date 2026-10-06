// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{collections::BTreeMap, error::Error, fmt};

pub const ASSET_PREFIX: &str = "/-/assets";

pub const ROBOTS_PATH: &str = "/robots.txt";
pub const FAVICON_ICO_PATH: &str = "/favicon.ico";
pub const FAVICON_SVG_PATH: &str = "/-/assets/favicon.svg";
pub const APPLE_TOUCH_ICON_PATH: &str = "/apple-touch-icon.png";
pub const MANIFEST_PATH: &str = "/-/assets/manifest.webmanifest";

pub const FONT_PREFIX: &str = "/-/assets/fonts";

/// How a generated asset is used by the server and HTML document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Stylesheet,
    Script,
    Static { content_type: &'static str },
}

impl AssetKind {
    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Stylesheet => "text/css; charset=utf-8",
            Self::Script => "text/javascript; charset=utf-8",
            Self::Static { content_type } => content_type,
        }
    }
}

/// Cache policy for an asset's canonical, fingerprinted name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CachePolicy {
    Revalidate,
    Immutable,
}

/// One build-generated, in-memory asset.
///
/// The original and canonical names, cache policy, bytes, and SHA-384 integrity value are emitted
/// together by `dimidiumlabs-ui-build`. Compiled CSS/JavaScript use a distinct xxHash64-
/// fingerprinted canonical name; copied source assets retain their logical name. The integrity
/// value belongs to the identity representation; encoded representations have distinct strong
/// validators because their octets differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Asset {
    kind: AssetKind,
    name: &'static str,
    fingerprinted_name: &'static str,
    cache: CachePolicy,
    bytes: &'static [u8],
    integrity: &'static str,
    gzip: Option<EncodedAsset>,
    brotli: Option<EncodedAsset>,
}

/// One precompressed representation of an [`Asset`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncodedAsset {
    bytes: &'static [u8],
    etag: &'static str,
}

impl Asset {
    /// Creates an entry emitted by `dimidiumlabs-ui-build`.
    ///
    /// Callers must keep `bytes`, the canonical name, and `integrity` from the same build output.
    /// [`AssetsCatalog`] deliberately validates metadata shape without hashing static bytes at
    /// runtime.
    #[doc(hidden)]
    #[must_use]
    pub const fn new(
        kind: AssetKind,
        name: &'static str,
        fingerprinted_name: &'static str,
        cache: CachePolicy,
        bytes: &'static [u8],
        integrity: &'static str,
    ) -> Self {
        Self {
            kind,
            name,
            fingerprinted_name,
            cache,
            bytes,
            integrity,
            gzip: None,
            brotli: None,
        }
    }

    /// Adds a build-time gzip representation and its representation-specific strong validator.
    #[doc(hidden)]
    #[must_use]
    pub const fn with_gzip(mut self, bytes: &'static [u8], etag: &'static str) -> Self {
        self.gzip = Some(EncodedAsset { bytes, etag });
        self
    }

    /// Adds a build-time Brotli representation and its representation-specific strong validator.
    #[doc(hidden)]
    #[must_use]
    pub const fn with_brotli(mut self, bytes: &'static [u8], etag: &'static str) -> Self {
        self.brotli = Some(EncodedAsset { bytes, etag });
        self
    }

    #[must_use]
    pub const fn kind(self) -> AssetKind {
        self.kind
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    #[must_use]
    pub const fn fingerprinted_name(self) -> &'static str {
        self.fingerprinted_name
    }

    #[must_use]
    pub const fn cache(self) -> CachePolicy {
        self.cache
    }

    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        self.bytes
    }

    #[must_use]
    pub const fn integrity(self) -> &'static str {
        self.integrity
    }

    /// Returns the build-time gzip representation, when retained by the size gate.
    #[must_use]
    pub const fn gzip(self) -> Option<EncodedAsset> {
        self.gzip
    }

    /// Returns the build-time Brotli representation, when retained by the size gate.
    #[must_use]
    pub const fn brotli(self) -> Option<EncodedAsset> {
        self.brotli
    }

    /// Returns whether this asset has any encoded representations.
    #[must_use]
    pub const fn has_encoded(self) -> bool {
        self.gzip.is_some() || self.brotli.is_some()
    }

    #[must_use]
    pub const fn content_type(self) -> &'static str {
        self.kind.content_type()
    }
}

impl EncodedAsset {
    #[must_use]
    pub const fn bytes(self) -> &'static [u8] {
        self.bytes
    }

    /// Returns the representation-specific strong `ETag` value, without HTTP quotes.
    #[must_use]
    pub const fn etag(self) -> &'static str {
        self.etag
    }
}

/// Ordered, transport-agnostic composition of generated assets.
///
/// The catalog validates names and metadata syntax. It trusts the build-generated digest-to-byte
/// relationship so constructing a catalog never hashes embedded asset bodies at runtime.
#[derive(Debug, Clone, Default)]
pub struct AssetsCatalog {
    assets: Vec<Asset>,
    names: BTreeMap<&'static str, AssetLookup>,
}

impl AssetsCatalog {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            assets: Vec::new(),
            names: BTreeMap::new(),
        }
    }

    /// Appends one generated asset set while retaining stylesheet and script order.
    ///
    /// # Errors
    /// Returns an error when names or integrity metadata are invalid, a distinct canonical name
    /// does not contain a 16-character lowercase hexadecimal fingerprint in the expected
    /// position, or either name duplicates an earlier entry.
    pub fn with(mut self, assets: &'static [Asset]) -> Result<Self, AssetsCatalogError> {
        for asset in assets {
            self.insert(*asset)?;
        }
        Ok(self)
    }

    #[must_use]
    pub fn assets(&self) -> &[Asset] {
        &self.assets
    }

    #[must_use]
    pub fn lookup(&self, name: &str) -> Option<AssetLookup> {
        self.names.get(name).copied()
    }

    pub fn stylesheets(&self) -> impl Iterator<Item = Asset> + '_ {
        self.assets
            .iter()
            .copied()
            .filter(|asset| asset.kind() == AssetKind::Stylesheet)
    }

    pub fn scripts(&self) -> impl Iterator<Item = Asset> + '_ {
        self.assets
            .iter()
            .copied()
            .filter(|asset| asset.kind() == AssetKind::Script)
    }

    fn insert(&mut self, asset: Asset) -> Result<(), AssetsCatalogError> {
        if !valid_name(asset.name()) {
            return Err(AssetsCatalogError::InvalidName(asset.name()));
        }
        if !valid_sha384_integrity(asset.integrity()) {
            return Err(AssetsCatalogError::InvalidIntegrity(asset.name()));
        }
        let mut representation_etags = [Some(asset.integrity()), None, None];
        for (index, representation) in [asset.gzip(), asset.brotli()].into_iter().enumerate() {
            if let Some(representation) = representation {
                if representation.bytes().is_empty()
                    || !valid_sha384_integrity(representation.etag())
                    || representation_etags[..=index]
                        .iter()
                        .flatten()
                        .any(|etag| *etag == representation.etag())
                {
                    return Err(AssetsCatalogError::InvalidIntegrity(asset.name()));
                }
                representation_etags[index + 1] = Some(representation.etag());
            }
        }
        if asset.name() != asset.fingerprinted_name()
            && !valid_fingerprinted_name(asset.name(), asset.fingerprinted_name())
        {
            return Err(AssetsCatalogError::InvalidFingerprint(asset.name()));
        }
        if self
            .names
            .insert(
                asset.name(),
                AssetLookup {
                    asset,
                    fingerprinted: false,
                },
            )
            .is_some()
        {
            return Err(AssetsCatalogError::DuplicateName(asset.name()));
        }
        if asset.name() != asset.fingerprinted_name()
            && self
                .names
                .insert(
                    asset.fingerprinted_name(),
                    AssetLookup {
                        asset,
                        fingerprinted: true,
                    },
                )
                .is_some()
        {
            return Err(AssetsCatalogError::DuplicateName(
                asset.fingerprinted_name(),
            ));
        }
        self.assets.push(asset);
        Ok(())
    }
}

/// Result of looking up either an original or fingerprinted asset name.
///
/// Build-generated CSS and JavaScript have distinct fingerprinted names. Copied source assets
/// retain their logical name in both fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetLookup {
    asset: Asset,
    fingerprinted: bool,
}

impl AssetLookup {
    #[must_use]
    pub const fn asset(self) -> Asset {
        self.asset
    }

    #[must_use]
    pub const fn is_fingerprinted(self) -> bool {
        self.fingerprinted
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssetsCatalogError {
    InvalidName(&'static str),
    InvalidIntegrity(&'static str),
    InvalidFingerprint(&'static str),
    DuplicateName(&'static str),
}

impl fmt::Display for AssetsCatalogError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName(name) => write!(formatter, "invalid asset name {name:?}"),
            Self::InvalidIntegrity(name) => {
                write!(formatter, "asset {name:?} has invalid integrity metadata")
            }
            Self::InvalidFingerprint(name) => {
                write!(formatter, "asset {name:?} has invalid fingerprint metadata")
            }
            Self::DuplicateName(name) => write!(formatter, "duplicate asset name {name:?}"),
        }
    }
}

impl Error for AssetsCatalogError {}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('/')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.'))
        && name
            .split('/')
            .all(|component| !component.is_empty() && !matches!(component, "." | ".."))
}

fn valid_sha384_integrity(value: &str) -> bool {
    value.strip_prefix("sha384-").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    })
}

fn valid_fingerprinted_name(name: &str, fingerprinted_name: &str) -> bool {
    let (directory, filename) = name
        .rsplit_once('/')
        .map_or(("", name), |(directory, filename)| (directory, filename));
    let (fingerprinted_directory, fingerprinted_filename) = fingerprinted_name
        .rsplit_once('/')
        .map_or(("", fingerprinted_name), |(directory, filename)| {
            (directory, filename)
        });
    if directory != fingerprinted_directory {
        return false;
    }
    let (stem, extension) = filename
        .rsplit_once('.')
        .map_or((filename, None), |(stem, extension)| {
            (stem, Some(extension))
        });
    let Some(suffix) = fingerprinted_filename.strip_prefix(stem) else {
        return false;
    };
    let suffix = match extension {
        Some(extension) => suffix.strip_suffix(&format!(".{extension}")),
        None => Some(suffix),
    };
    suffix.is_some_and(|suffix| {
        suffix.strip_prefix('.').is_some_and(|hash| {
            hash.len() == 16
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_asset_keeps_all_build_time_metadata_together() {
        const ASSET: Asset = Asset::new(
            AssetKind::Script,
            "app.js",
            "app.0123456789abcdef.js",
            CachePolicy::Immutable,
            b"console.log('ok')",
            "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        )
        .with_gzip(
            b"gzip",
            "sha384-ts70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        )
        .with_brotli(
            b"brotli",
            "sha384-vs70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        );
        assert_eq!(ASSET.kind(), AssetKind::Script);
        assert_eq!(ASSET.name(), "app.js");
        assert_eq!(ASSET.fingerprinted_name(), "app.0123456789abcdef.js");
        assert_eq!(ASSET.cache(), CachePolicy::Immutable);
        assert!(ASSET.integrity().starts_with("sha384-"));
        assert_eq!(ASSET.bytes(), b"console.log('ok')");
        assert_eq!(ASSET.gzip().unwrap().bytes(), b"gzip");
        assert_eq!(ASSET.brotli().unwrap().bytes(), b"brotli");
        assert!(ASSET.has_encoded());
    }

    #[test]
    fn catalog_composes_in_order_and_looks_up_both_generated_names() {
        const FOUNDATION: &[Asset] = &[Asset::new(
            AssetKind::Stylesheet,
            "global.css",
            "global.0000000000000001.css",
            CachePolicy::Immutable,
            b"global",
            "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        )];
        const APPLICATION: &[Asset] = &[
            Asset::new(
                AssetKind::Stylesheet,
                "app.css",
                "app.0000000000000002.css",
                CachePolicy::Immutable,
                b"app",
                "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
            ),
            Asset::new(
                AssetKind::Static {
                    content_type: "image/png",
                },
                "icon.png",
                "icon.png",
                CachePolicy::Revalidate,
                b"icon",
                "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
            ),
        ];
        let catalog = AssetsCatalog::new()
            .with(FOUNDATION)
            .unwrap()
            .with(APPLICATION)
            .unwrap();
        assert_eq!(
            catalog.stylesheets().map(Asset::name).collect::<Vec<_>>(),
            ["global.css", "app.css"]
        );
        assert!(catalog.lookup("global.css").is_some());
        assert!(
            catalog
                .lookup("global.0000000000000001.css")
                .unwrap()
                .is_fingerprinted()
        );
        assert!(!catalog.lookup("icon.png").unwrap().is_fingerprinted());
    }

    #[test]
    fn catalog_rejects_duplicate_and_malformed_generated_metadata() {
        const VALID: Asset = Asset::new(
            AssetKind::Stylesheet,
            "app.css",
            "app.0000000000000001.css",
            CachePolicy::Immutable,
            b"app",
            "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        );
        const INVALID: Asset = Asset::new(
            AssetKind::Stylesheet,
            "other.css",
            "other.short.css",
            CachePolicy::Immutable,
            b"other",
            "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        );
        assert!(matches!(
            AssetsCatalog::new().with(&[VALID, VALID]),
            Err(AssetsCatalogError::DuplicateName("app.css"))
        ));
        assert!(matches!(
            AssetsCatalog::new().with(&[INVALID]),
            Err(AssetsCatalogError::InvalidFingerprint("other.css"))
        ));
    }
}
