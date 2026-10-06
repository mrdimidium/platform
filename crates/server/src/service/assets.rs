// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Request, Response, StatusCode, header},
};
use dimidiumlabs_ui::{AssetsCatalog, CachePolicy};
use tower::{Layer, Service};

use crate::assets::lookup_uri;

use super::html::if_none_match_matches;
use crate::body::empty_unknown_size;

/// Applies build-generated strong validators, cache policy, and asset-specific security headers
/// without buffering response bodies.
#[derive(Debug, Clone)]
pub struct AssetsLayer {
    catalog: Arc<AssetsCatalog>,
    aliases: Arc<BTreeMap<String, String>>,
}

impl AssetsLayer {
    #[must_use]
    pub fn new(catalog: Arc<AssetsCatalog>) -> Self {
        Self {
            catalog,
            aliases: Arc::new(BTreeMap::new()),
        }
    }

    pub(crate) fn with_alias(mut self, path: &str, name: &str) -> Self {
        Arc::make_mut(&mut self.aliases).insert(path.to_owned(), name.to_owned());
        self
    }
}

impl<S> Layer<S> for AssetsLayer {
    type Service = AssetsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AssetsService {
            inner,
            catalog: Arc::clone(&self.catalog),
            aliases: Arc::clone(&self.aliases),
        }
    }
}

/// Service produced by [`AssetsLayer`].
#[derive(Debug, Clone)]
pub struct AssetsService<S> {
    inner: S,
    catalog: Arc<AssetsCatalog>,
    aliases: Arc<BTreeMap<String, String>>,
}

impl<S> Service<Request<Body>> for AssetsService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        let method = request.method().clone();
        let if_none_match = request.headers().get(header::IF_NONE_MATCH).cloned();
        let asset = lookup_uri(&self.catalog, request.uri().path()).or_else(|| {
            self.aliases
                .get(request.uri().path())
                .and_then(|name| self.catalog.lookup(name))
        });
        let replacement = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, replacement);

        Box::pin(async move {
            let response = inner.call(request).await?;
            let (mut parts, body) = response.into_parts();
            parts.headers.insert(
                header::X_CONTENT_TYPE_OPTIONS,
                HeaderValue::from_static("nosniff"),
            );
            parts.headers.insert(
                HeaderName::from_static("cross-origin-resource-policy"),
                HeaderValue::from_static("same-origin"),
            );
            parts
                .headers
                .entry(header::CACHE_CONTROL)
                .or_insert(HeaderValue::from_static("private, no-cache"));

            if parts.status == StatusCode::OK
                && let Some(asset) = asset
            {
                let cache_control = match asset.asset().cache() {
                    CachePolicy::Immutable => {
                        HeaderValue::from_static("public, max-age=31536000, immutable")
                    }
                    CachePolicy::Revalidate => HeaderValue::from_static("no-cache"),
                };
                parts.headers.insert(header::CACHE_CONTROL, cache_control);
                let etag = parts
                    .headers
                    .get(header::ETAG)
                    .and_then(|value| value.to_str().ok())
                    .map_or_else(
                        || format!("\"{}\"", asset.asset().integrity()),
                        str::to_owned,
                    );
                parts.headers.insert(
                    header::ETAG,
                    HeaderValue::from_str(&etag)
                        .expect("catalog-validated integrity is a valid strong ETag"),
                );

                if matches!(method, axum::http::Method::GET | axum::http::Method::HEAD)
                    && if_none_match
                        .as_ref()
                        .is_some_and(|value| if_none_match_matches(value, &etag))
                {
                    parts.status = StatusCode::NOT_MODIFIED;
                    parts.headers.remove(header::CONTENT_LENGTH);
                    return Ok(Response::from_parts(parts, empty_unknown_size()));
                }
                if method == axum::http::Method::HEAD {
                    return Ok(Response::from_parts(parts, Body::empty()));
                }
            }
            Ok(Response::from_parts(parts, body))
        })
    }
}
