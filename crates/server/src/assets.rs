// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::Extension,
    http::{HeaderMap, Response, StatusCode, Uri, header},
    routing::get,
};
use dimidiumlabs_ui::{
    APPLE_TOUCH_ICON_PATH, ASSET_PREFIX, Asset, AssetLookup, AssetsCatalog, EncodedAsset,
    FAVICON_ICO_PATH, ROBOTS_PATH,
};

use crate::service::AssetsLayer;

/// Builds the Axum serving adapter for one composed, transport-agnostic asset catalog.
pub fn assets_router<S>(catalog: Arc<AssetsCatalog>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(FAVICON_ICO_PATH, get(serve_asset))
        .route(ROBOTS_PATH, get(serve_asset))
        .route(APPLE_TOUCH_ICON_PATH, get(serve_asset))
        .route("/-/assets/{*path}", get(serve_asset))
        .layer(AssetsLayer::new(Arc::clone(&catalog)))
        .layer(Extension(catalog))
}

/// Builds an Axum serving adapter for one catalog asset at an application-selected route.
///
/// This is useful for deterministic build outputs which need to retain an established URL outside
/// [`dimidiumlabs_ui::ASSET_PREFIX`]. The named asset receives the same negotiation, validator,
/// cache, security, `HEAD`, and conditional-request behavior as [`assets_router`].
pub fn asset_router_at<S>(path: &str, name: &'static str, catalog: Arc<AssetsCatalog>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route(path, get(serve_named_asset))
        .layer(AssetsLayer::new(Arc::clone(&catalog)).with_alias(path, name))
        .layer(Extension(NamedAsset(name)))
        .layer(Extension(catalog))
}

#[derive(Clone, Copy)]
struct NamedAsset(&'static str);

pub(crate) fn lookup_uri(catalog: &AssetsCatalog, path: &str) -> Option<AssetLookup> {
    let name = match path {
        FAVICON_ICO_PATH => "favicon.ico",
        ROBOTS_PATH => "robots.txt",
        APPLE_TOUCH_ICON_PATH => "apple-touch-icon.png",
        _ => path.strip_prefix(ASSET_PREFIX)?.strip_prefix('/')?,
    };
    let asset = catalog.lookup(name)?;
    if asset.asset().name() != asset.asset().fingerprinted_name() && !asset.is_fingerprinted() {
        return None;
    }
    Some(asset)
}

async fn serve_asset(
    Extension(catalog): Extension<Arc<AssetsCatalog>>,
    headers: HeaderMap,
    uri: Uri,
) -> Response<Body> {
    let asset = lookup_uri(&catalog, uri.path()).map(AssetLookup::asset);
    serve_asset_representation(asset, &headers)
}

async fn serve_named_asset(
    Extension(catalog): Extension<Arc<AssetsCatalog>>,
    Extension(NamedAsset(name)): Extension<NamedAsset>,
    headers: HeaderMap,
) -> Response<Body> {
    let asset = catalog.lookup(name).map(AssetLookup::asset);
    serve_asset_representation(asset, &headers)
}

fn serve_asset_representation(asset: Option<Asset>, headers: &HeaderMap) -> Response<Body> {
    let Some(asset) = asset else {
        return Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .expect("empty not-found response is valid");
    };

    let Some(representation) = negotiate(asset, headers) else {
        let mut response = Response::builder().status(StatusCode::NOT_ACCEPTABLE);
        if let Some(vary) = vary_for_asset(asset, headers) {
            response = response.header(header::VARY, vary);
        }
        return response
            .body(Body::empty())
            .expect("empty not-acceptable response is valid");
    };

    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, asset.content_type())
        .header(header::CONTENT_LENGTH, representation.bytes.len())
        .header(header::ETAG, format!("\"{}\"", representation.etag));
    if let Some(vary) = vary_for_asset(asset, headers) {
        response = response.header(header::VARY, vary);
    }
    if let Some(encoding) = representation.encoding {
        response = response.header(header::CONTENT_ENCODING, encoding);
    }
    response
        .body(Body::from(representation.bytes))
        .expect("registered UI asset response is valid")
}

fn vary_for_asset(asset: Asset, headers: &HeaderMap) -> Option<&'static str> {
    if !asset.has_encoded() {
        None
    } else if headers.contains_key(header::RANGE) {
        Some("Accept-Encoding, Range")
    } else {
        Some("Accept-Encoding")
    }
}

#[derive(Clone, Copy)]
struct Representation {
    bytes: &'static [u8],
    etag: &'static str,
    encoding: Option<&'static str>,
}

impl Representation {
    const fn identity(asset: Asset) -> Self {
        Self {
            bytes: asset.bytes(),
            etag: asset.integrity(),
            encoding: None,
        }
    }

    const fn encoded(asset: EncodedAsset, encoding: &'static str) -> Self {
        Self {
            bytes: asset.bytes(),
            etag: asset.etag(),
            encoding: Some(encoding),
        }
    }
}

fn negotiate(asset: Asset, headers: &HeaderMap) -> Option<Representation> {
    let accepted = AcceptedEncodings::parse(headers);
    let allow_encoded = !headers.contains_key(header::RANGE);
    let mut selected: Option<(u16, Representation)> = None;
    for (quality, representation) in [
        asset
            .brotli()
            .filter(|_| allow_encoded)
            .map(|representation| {
                (
                    accepted.brotli(),
                    Representation::encoded(representation, "br"),
                )
            }),
        asset
            .gzip()
            .filter(|_| allow_encoded)
            .map(|representation| {
                (
                    accepted.gzip(),
                    Representation::encoded(representation, "gzip"),
                )
            }),
        Some((accepted.identity(), Representation::identity(asset))),
    ]
    .into_iter()
    .flatten()
    {
        if quality == 0 {
            continue;
        }
        if selected.is_none_or(|(best, _)| quality > best) {
            selected = Some((quality, representation));
        }
    }
    selected.map(|(_, representation)| representation)
}

#[derive(Default)]
struct AcceptedEncodings {
    present: bool,
    brotli: Option<u16>,
    gzip: Option<u16>,
    identity: Option<u16>,
    wildcard: Option<u16>,
}

impl AcceptedEncodings {
    fn parse(headers: &HeaderMap) -> Self {
        let mut accepted = Self::default();
        for value in headers.get_all(header::ACCEPT_ENCODING) {
            accepted.present = true;
            let Ok(value) = value.to_str() else {
                continue;
            };
            for item in value.split(',') {
                let mut parts = item.split(';');
                let coding = parts.next().unwrap_or_default().trim();
                if coding.is_empty() {
                    continue;
                }
                let mut quality = 1_000;
                for parameter in parts {
                    let Some((name, value)) = parameter.split_once('=') else {
                        continue;
                    };
                    if name.trim().eq_ignore_ascii_case("q") {
                        quality = parse_quality(value.trim()).unwrap_or(0);
                    }
                }
                let slot = if coding.eq_ignore_ascii_case("br") {
                    &mut accepted.brotli
                } else if coding.eq_ignore_ascii_case("gzip") {
                    &mut accepted.gzip
                } else if coding.eq_ignore_ascii_case("identity") {
                    &mut accepted.identity
                } else if coding == "*" {
                    &mut accepted.wildcard
                } else {
                    continue;
                };
                *slot = Some(slot.map_or(quality, |previous| previous.max(quality)));
            }
        }
        accepted
    }

    fn brotli(&self) -> u16 {
        self.coding(self.brotli)
    }

    fn gzip(&self) -> u16 {
        self.coding(self.gzip)
    }

    fn coding(&self, explicit: Option<u16>) -> u16 {
        if self.present {
            explicit.or(self.wildcard).unwrap_or(0)
        } else {
            0
        }
    }

    fn identity(&self) -> u16 {
        if !self.present {
            return 1_000;
        }
        self.identity
            .unwrap_or_else(|| u16::from(self.wildcard != Some(0)) * 1_000)
    }
}

fn parse_quality(value: &str) -> Option<u16> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match whole {
        "0" => {
            let mut quality = 0_u16;
            let mut multiplier = 100;
            for digit in fraction.bytes() {
                quality += u16::from(digit - b'0') * multiplier;
                multiplier /= 10;
            }
            Some(quality)
        }
        "1" if fraction.bytes().all(|byte| byte == b'0') => Some(1_000),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::Request;
    use dimidiumlabs_ui::{AssetKind, CachePolicy};
    use http_body_util::BodyExt as _;
    use tower::ServiceExt as _;

    use super::*;

    const IDENTITY_ETAG: &str =
        "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS";
    const GZIP_ETAG: &str =
        "sha384-ts70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS";
    const BROTLI_ETAG: &str =
        "sha384-vs70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS";
    const ASSETS: &[Asset] = &[
        Asset::new(
            AssetKind::Stylesheet,
            "app.css",
            "app.0123456789abcdef.css",
            CachePolicy::Immutable,
            b"identity",
            IDENTITY_ETAG,
        )
        .with_gzip(b"gzip", GZIP_ETAG)
        .with_brotli(b"brotli", BROTLI_ETAG),
        Asset::new(
            AssetKind::Static {
                content_type: "image/x-icon",
            },
            "favicon.ico",
            "favicon.ico",
            CachePolicy::Revalidate,
            b"icon",
            IDENTITY_ETAG,
        ),
    ];

    fn catalog() -> Arc<AssetsCatalog> {
        Arc::new(AssetsCatalog::new().with(ASSETS).unwrap())
    }

    #[test]
    fn maps_original_fingerprinted_and_conventional_root_names() {
        let catalog = catalog();
        assert!(lookup_uri(&catalog, "/-/assets/app.css").is_none());
        let fingerprinted = lookup_uri(&catalog, "/-/assets/app.0123456789abcdef.css").unwrap();
        assert!(fingerprinted.is_fingerprinted());
        assert_eq!(
            lookup_uri(&catalog, FAVICON_ICO_PATH)
                .unwrap()
                .asset()
                .name(),
            "favicon.ico"
        );
        assert!(lookup_uri(&catalog, "/outside/app.css").is_none());
    }

    #[test]
    fn negotiates_quality_wildcards_and_identity() {
        let asset = ASSETS[0];
        let headers = HeaderMap::new();
        assert_eq!(negotiate(asset, &headers).unwrap().encoding, None);

        let mut headers = HeaderMap::new();
        headers.insert(header::ACCEPT_ENCODING, "gzip, br".parse().unwrap());
        assert_eq!(negotiate(asset, &headers).unwrap().encoding, Some("br"));

        headers.insert(
            header::ACCEPT_ENCODING,
            "br;q=0.4, gzip;q=0.8, identity;q=0.5".parse().unwrap(),
        );
        assert_eq!(negotiate(asset, &headers).unwrap().encoding, Some("gzip"));

        headers.insert(
            header::ACCEPT_ENCODING,
            "*;q=0, identity;q=0".parse().unwrap(),
        );
        assert!(negotiate(asset, &headers).is_none());

        headers.insert(header::ACCEPT_ENCODING, "br".parse().unwrap());
        headers.insert(header::RANGE, "bytes=0-3".parse().unwrap());
        assert_eq!(negotiate(asset, &headers).unwrap().encoding, None);
        assert_eq!(
            vary_for_asset(asset, &headers),
            Some("Accept-Encoding, Range")
        );

        let mut headers = HeaderMap::new();
        headers.append(
            header::ACCEPT_ENCODING,
            "gzip;q=0.4, identity;q=0.5".parse().unwrap(),
        );
        headers.append(header::ACCEPT_ENCODING, "gzip;q=0.8".parse().unwrap());
        assert_eq!(negotiate(asset, &headers).unwrap().encoding, Some("gzip"));

        headers.clear();
        headers.insert(
            header::ACCEPT_ENCODING,
            "*;q=1, gzip;q=0, identity;q=0".parse().unwrap(),
        );
        assert_eq!(negotiate(asset, &headers).unwrap().encoding, Some("br"));

        headers.insert(
            header::ACCEPT_ENCODING,
            "br;q=invalid, gzip;q=0, identity;q=0".parse().unwrap(),
        );
        assert!(negotiate(asset, &headers).is_none());
    }

    #[test]
    fn parses_quality_without_floating_point() {
        assert_eq!(parse_quality("0"), Some(0));
        assert_eq!(parse_quality("0.5"), Some(500));
        assert_eq!(parse_quality("0.025"), Some(25));
        assert_eq!(parse_quality("1.000"), Some(1_000));
        assert_eq!(parse_quality("1.001"), None);
        assert_eq!(parse_quality(".5"), None);
        assert_eq!(parse_quality("0.0000"), None);
    }

    #[tokio::test]
    async fn serves_named_catalog_assets_at_application_routes() {
        let app = asset_router_at::<()>("/-/licenses.json", "app.css", catalog());
        let response = app
            .oneshot(
                Request::get("/-/licenses.json")
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(response.headers()[header::ETAG], format!("\"{GZIP_ETAG}\""));
    }

    #[tokio::test]
    async fn serves_representation_headers_head_and_conditional_requests() {
        let app = assets_router::<()>(catalog());
        let path = "/-/assets/app.0123456789abcdef.css";
        let response = app
            .clone()
            .oneshot(
                Request::get(path)
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "4");
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        assert_eq!(response.headers()[header::ETAG], format!("\"{GZIP_ETAG}\""));
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "gzip"
        );

        let response = app
            .clone()
            .oneshot(
                Request::head(path)
                    .header(header::ACCEPT_ENCODING, "br")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(response.headers()[header::CONTENT_LENGTH], "6");
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );

        let response = app
            .oneshot(
                Request::get(path)
                    .header(header::ACCEPT_ENCODING, "br")
                    .header(header::IF_NONE_MATCH, format!("W/\"{BROTLI_ETAG}\""))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        assert!(
            !response.headers().contains_key(header::CONTENT_LENGTH),
            "unexpected headers: {:?}",
            response.headers()
        );
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
    }
}
