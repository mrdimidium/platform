// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

mod assets;
mod body;
mod html_compression;
pub mod service;
pub mod tls;
pub mod transport;

pub use assets::{asset_router_at, assets_router};
pub use html_compression::HtmlCompressionPredicate;

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        Router,
        body::{Body, to_bytes},
        http::{Method, Request, StatusCode, header},
        routing::get,
    };
    use dimidiumlabs_ui::{ASSET_PREFIX, Asset, AssetKind, AssetsCatalog, CachePolicy, FOUNDATION};
    use tower::ServiceExt;

    use crate::{
        assets_router,
        service::{HtmlLayer, content_security_policy_for_scripts},
    };

    const APP_STYLESHEET_INTEGRITY: &str =
        "sha384-SowSEdgnLzQ3w/9nX1OP2UUcL0/AMItyO5QPGJEVYviRZ81zbHfHZrG3XZAsR0mk";
    const APPLICATION: &[Asset] = &[
        Asset::new(
            AssetKind::Stylesheet,
            "app.css",
            "app.0123456789abcdef.css",
            CachePolicy::Immutable,
            b"body{color:black}",
            APP_STYLESHEET_INTEGRITY,
        ),
        Asset::new(
            AssetKind::Script,
            "app.js",
            "app.0123456789abcdef.js",
            CachePolicy::Immutable,
            b"console.log('ok')",
            "sha384-us70yumzLF2TpSodT8MxNxbLn5LCPNNUhaDDHTqXZZcBW+y9KnFu0zoe9CWl0mvS",
        ),
    ];

    fn catalog() -> Arc<AssetsCatalog> {
        Arc::new(
            AssetsCatalog::new()
                .with(FOUNDATION)
                .unwrap()
                .with(APPLICATION)
                .unwrap(),
        )
    }

    fn router() -> Router {
        let catalog = catalog();
        assets_router(Arc::clone(&catalog)).layer(HtmlLayer::new(&catalog))
    }

    #[tokio::test]
    async fn assets_keep_get_head_etag_cache_and_security_contract() {
        let router = router();
        let path = format!("{ASSET_PREFIX}/app.0123456789abcdef.css");
        let response = router
            .clone()
            .oneshot(Request::builder().uri(&path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_none()
        );
        let etag = response.headers()[header::ETAG].clone();
        assert_eq!(etag, format!("\"{APP_STYLESHEET_INTEGRITY}\""));
        let length = response.headers()[header::CONTENT_LENGTH].clone();

        let head = router
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::HEAD)
                    .uri(&path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::ETAG], etag);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], length);
        assert!(
            to_bytes(head.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );

        let cached = router
            .oneshot(
                Request::builder()
                    .uri(&path)
                    .header(header::IF_NONE_MATCH, &etag)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cached.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(cached.headers()[header::ETAG], etag);
    }

    #[tokio::test]
    async fn fingerprinted_assets_are_immutable_and_use_integrity_as_strong_etag() {
        let catalog = catalog();
        let asset = catalog.lookup("app.css").unwrap().asset();
        let path = format!("{ASSET_PREFIX}/{}", asset.fingerprinted_name());
        let router = assets_router(Arc::clone(&catalog))
            .layer(HtmlLayer::new(&catalog).with_max_body_bytes(0));
        let response = router
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert_eq!(
            response.headers()[header::ETAG],
            format!("\"{APP_STYLESHEET_INTEGRITY}\"")
        );
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "body{color:black}"
        );
    }

    #[tokio::test]
    async fn fingerprinted_foundation_fonts_are_immutable() {
        let catalog = catalog();
        let font = catalog
            .assets()
            .iter()
            .find(|asset| {
                std::path::Path::new(asset.name())
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("woff2"))
            })
            .unwrap();
        let path = format!("{ASSET_PREFIX}/{}", font.fingerprinted_name());
        let response = router()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        assert!(
            response.headers()[header::ETAG]
                .to_str()
                .unwrap()
                .starts_with("\"sha384-")
        );
    }

    #[tokio::test]
    async fn html_layer_leaves_non_html_bodies_streaming_and_untouched() {
        let catalog = catalog();
        let router = Router::new()
            .route(
                "/",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "application/octet-stream")],
                        "streaming",
                    )
                }),
            )
            .layer(HtmlLayer::new(&catalog).with_max_body_bytes(0));
        let response = router.oneshot(Request::new(Body::empty())).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_none()
        );
        assert_eq!(
            to_bytes(response.into_body(), usize::MAX).await.unwrap(),
            "streaming"
        );
    }

    #[tokio::test]
    async fn bounded_html_buffering_fails_closed_with_catalog_csp() {
        let catalog = catalog();
        let bounded = Router::new()
            .route(
                "/",
                get(|| async {
                    (
                        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                        "too large",
                    )
                }),
            )
            .layer(HtmlLayer::new(&catalog).with_max_body_bytes(3));
        let response = bounded.oneshot(Request::new(Body::empty())).await.unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            content_security_policy_for_scripts(catalog.scripts().map(Asset::integrity))
        );
        assert!(
            to_bytes(response.into_body(), usize::MAX)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn asset_headers_are_kept_on_not_found_responses() {
        let response = router()
            .oneshot(
                Request::builder()
                    .uri("/-/assets/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response.headers()[header::CACHE_CONTROL],
            "private, no-cache"
        );
        assert_eq!(
            response.headers()[header::X_CONTENT_TYPE_OPTIONS],
            "nosniff"
        );
        assert_eq!(
            response.headers()["cross-origin-resource-policy"],
            "same-origin"
        );
        assert!(
            response
                .headers()
                .get(header::CONTENT_SECURITY_POLICY)
                .is_none()
        );
    }
}
