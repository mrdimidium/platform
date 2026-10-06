// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    fmt::Write as _,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request, Response, StatusCode, header},
};
use dimidiumlabs_ui::{Asset, AssetsCatalog};
use sha2::{Digest as _, Sha256};
use tower::{Layer, Service};

use crate::body::empty_unknown_size;

/// Applies HTML-specific security, cache, and validator policy.
#[derive(Debug, Clone)]
pub struct HtmlLayer {
    max_body_bytes: usize,
    content_security_policy: HeaderValue,
    negotiated_compression: bool,
}

impl HtmlLayer {
    #[must_use]
    pub fn new(catalog: &AssetsCatalog) -> Self {
        Self {
            content_security_policy: content_security_policy_for_scripts(
                catalog.scripts().map(Asset::integrity),
            ),
            max_body_bytes: DEFAULT_MAX_HTML_BODY_BYTES,
            negotiated_compression: false,
        }
    }

    /// Sets the maximum successful HTML GET/HEAD response body buffered for `ETag` calculation.
    #[must_use]
    pub const fn with_max_body_bytes(mut self, max_body_bytes: usize) -> Self {
        self.max_body_bytes = max_body_bytes;
        self
    }

    /// Marks HTML validators weak and varies caches by `Accept-Encoding`.
    ///
    /// Enable this when an outer streaming compression layer can select an encoded
    /// representation. This method changes only validator/cache metadata; it does not compress or
    /// buffer the response.
    #[must_use]
    pub const fn with_negotiated_compression(mut self) -> Self {
        self.negotiated_compression = true;
        self
    }
}

impl<S> Layer<S> for HtmlLayer {
    type Service = HtmlService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HtmlService {
            inner,
            max_body_bytes: self.max_body_bytes,
            content_security_policy: self.content_security_policy.clone(),
            negotiated_compression: self.negotiated_compression,
        }
    }
}

/// Service produced by [`HtmlLayer`].
#[derive(Debug, Clone)]
pub struct HtmlService<S> {
    inner: S,
    max_body_bytes: usize,
    content_security_policy: HeaderValue,
    negotiated_compression: bool,
}

impl<S> Service<Request<Body>> for HtmlService<S>
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
        let max_body_bytes = self.max_body_bytes;
        let content_security_policy = self.content_security_policy.clone();
        let negotiated_compression = self.negotiated_compression;

        let method = request.method().clone();
        let if_none_match = request.headers().get(header::IF_NONE_MATCH).cloned();

        let replacement = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, replacement);

        Box::pin(async move {
            let response = inner.call(request).await?;
            Ok(harden_html_response(
                method,
                if_none_match,
                response,
                &content_security_policy,
                max_body_bytes,
                negotiated_compression,
            )
            .await)
        })
    }
}

pub const DEFAULT_MAX_HTML_BODY_BYTES: usize = 8 * 1024 * 1024;

pub const POLICY_PERMISSIONS: &str = concat!(
    "accelerometer=(), ",
    "magnetometer=(), ",
    "geolocation=(), ",
    "microphone=(), ",
    "gyroscope=(), ",
    "payment=(), ",
    "camera=(), ",
    "usb=()",
);
pub const POLICY_CSP: &str = concat!(
    "base-uri 'none'; ",
    "default-src 'self'; ",
    "img-src 'self'; ",
    "font-src 'self'; ",
    "style-src 'self'; ",
    "connect-src 'self'; ",
    "form-action 'self'; ",
    "manifest-src 'self'; ",
    "frame-ancestors 'none'; ",
    "object-src 'none'; ",
);
const CROSS_ORIGIN_OPENER_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-opener-policy");
const CROSS_ORIGIN_RESOURCE_POLICY: HeaderName =
    HeaderName::from_static("cross-origin-resource-policy");
const PERMISSIONS_POLICY_HEADER: HeaderName = HeaderName::from_static("permissions-policy");

/// Builds a CSP that authorizes only scripts carrying one of the supplied SHA-384 SRI values.
///
/// An empty iterator produces `script-src 'none'`. Values that are not valid SHA-384 SRI metadata
/// are ignored rather than interpolated into the policy.
///
/// # Panics
/// Panics only if the internally generated ASCII policy cannot be represented as a header value.
#[must_use]
pub fn content_security_policy_for_scripts<'a>(
    integrities: impl IntoIterator<Item = &'a str>,
) -> HeaderValue {
    let mut policy = String::from(POLICY_CSP);
    policy.push_str("script-src");

    let mut found = false;
    for integrity in integrities {
        if !valid_sha384_integrity(integrity) {
            continue;
        }
        found = true;
        policy.push_str(" '");
        policy.push_str(integrity);
        policy.push('\'');
    }
    if !found {
        policy.push_str(" 'none'");
    }

    HeaderValue::from_str(&policy).expect("generated content security policy is valid ASCII")
}

pub(crate) fn valid_sha384_integrity(value: &str) -> bool {
    value.strip_prefix("sha384-").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/'))
    })
}

async fn harden_html_response(
    method: Method,
    if_none_match: Option<HeaderValue>,
    response: Response<Body>,
    content_security_policy: &HeaderValue,
    max_body_bytes: usize,
    negotiated_compression: bool,
) -> Response<Body> {
    if !is_html_response(&response) {
        return response;
    }
    let status = response.status();
    let (mut parts, body) = response.into_parts();
    apply_html_headers(
        &mut parts.headers,
        content_security_policy,
        negotiated_compression,
    );
    if negotiated_compression {
        weaken_etag(&mut parts.headers);
    }
    if status != StatusCode::OK || !matches!(method, Method::GET | Method::HEAD) {
        return Response::from_parts(parts, body);
    }

    if let Some(etag_header) = parts.headers.get(header::ETAG).cloned() {
        if etag_header.to_str().is_ok_and(|etag| {
            if_none_match
                .as_ref()
                .is_some_and(|value| if_none_match_matches(value, etag))
        }) {
            parts.status = StatusCode::NOT_MODIFIED;
            parts.headers.remove(header::CONTENT_LENGTH);
            return Response::from_parts(parts, empty_unknown_size());
        }
        let body = if method == Method::HEAD {
            Body::empty()
        } else {
            body
        };
        return Response::from_parts(parts, body);
    }

    let bytes = match axum::body::to_bytes(body, max_body_bytes).await {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("failed to buffer bounded HTML response: {error}");
            parts.status = StatusCode::INTERNAL_SERVER_ERROR;
            parts.headers.remove(header::ETAG);
            parts.headers.remove(header::CONTENT_LENGTH);
            parts
                .headers
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            return Response::from_parts(parts, Body::empty());
        }
    };

    let etag = if negotiated_compression {
        format!("W/{}", strong_etag(&bytes))
    } else {
        strong_etag(&bytes)
    };
    let etag_header = HeaderValue::from_str(&etag).expect("SHA-256 entity tag is valid ASCII");
    if if_none_match
        .as_ref()
        .is_some_and(|value| if_none_match_matches(value, &etag))
    {
        parts.status = StatusCode::NOT_MODIFIED;
        parts.headers.insert(header::ETAG, etag_header);
        parts.headers.remove(header::CONTENT_LENGTH);
        return Response::from_parts(parts, empty_unknown_size());
    }

    parts.headers.insert(header::ETAG, etag_header);
    parts.headers.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&bytes.len().to_string()).expect("response length is valid"),
    );
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(bytes)
    };
    Response::from_parts(parts, body)
}

fn apply_html_headers(
    headers: &mut axum::http::HeaderMap,
    content_security_policy: &HeaderValue,
    negotiated_compression: bool,
) {
    if negotiated_compression
        && !headers
            .get_all(header::VARY)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .flat_map(|value| value.split(','))
            .any(|name| {
                let name = name.trim();
                name == "*" || name.eq_ignore_ascii_case(header::ACCEPT_ENCODING.as_str())
            })
    {
        headers.append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
    headers
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("private, no-cache"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        content_security_policy.clone(),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        CROSS_ORIGIN_OPENER_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        CROSS_ORIGIN_RESOURCE_POLICY,
        HeaderValue::from_static("same-origin"),
    );
    headers.insert(
        PERMISSIONS_POLICY_HEADER,
        HeaderValue::from_static(POLICY_PERMISSIONS),
    );
}

fn weaken_etag(headers: &mut axum::http::HeaderMap) {
    let Some(etag) = headers.get(header::ETAG) else {
        return;
    };
    let bytes = etag.as_bytes();
    if bytes.starts_with(b"W/\"") || !bytes.starts_with(b"\"") || !bytes.ends_with(b"\"") {
        return;
    }
    let mut weak = Vec::with_capacity(bytes.len() + 2);
    weak.extend_from_slice(b"W/");
    weak.extend_from_slice(bytes);
    headers.insert(
        header::ETAG,
        HeaderValue::from_bytes(&weak).expect("valid strong ETag remains valid when weakened"),
    );
}

fn is_html_response(response: &Response<Body>) -> bool {
    response
        .headers()
        .get(header::CONTENT_TYPE)
        .is_some_and(|value| {
            value
                .as_bytes()
                .split(|byte| *byte == b';')
                .next()
                .is_some_and(|value| value.eq_ignore_ascii_case(b"text/html"))
        })
}

pub(crate) fn strong_etag(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut etag = String::with_capacity(2 + digest.len() * 2);
    etag.push('"');
    for byte in digest {
        write!(etag, "{byte:02x}").expect("writing to String cannot fail");
    }
    etag.push('"');
    etag
}

pub(crate) fn if_none_match_matches(header: &HeaderValue, etag: &str) -> bool {
    let Ok(value) = header.to_str() else {
        return false;
    };
    let etag = etag.strip_prefix("W/").unwrap_or(etag);
    value.split(',').any(|candidate| {
        let candidate = candidate.trim();
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn negotiated_compression_uses_weak_etags_and_varies_304() {
        let body = b"dynamic html";
        let expected = format!("W/{}", strong_etag(body));
        let response = harden_html_response(
            Method::GET,
            None,
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(body.as_slice()))
                .unwrap(),
            &HeaderValue::from_static("default-src 'none'"),
            DEFAULT_MAX_HTML_BODY_BYTES,
            true,
        )
        .await;
        assert_eq!(response.headers()[header::ETAG], expected);
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");

        let response = harden_html_response(
            Method::GET,
            Some(HeaderValue::from_str(expected.trim_start_matches("W/")).unwrap()),
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(body.as_slice()))
                .unwrap(),
            &HeaderValue::from_static("default-src 'none'"),
            DEFAULT_MAX_HTML_BODY_BYTES,
            true,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(response.headers()[header::ETAG], expected);
        assert_eq!(response.headers()[header::VARY], "Accept-Encoding");
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
    }

    #[test]
    fn if_none_match_uses_weak_comparison() {
        assert!(if_none_match_matches(
            &HeaderValue::from_static("W/\"current\""),
            "W/\"current\""
        ));
        assert!(if_none_match_matches(
            &HeaderValue::from_static("\"current\""),
            "W/\"current\""
        ));
        assert!(if_none_match_matches(
            &HeaderValue::from_static("W/\"current\""),
            "\"current\""
        ));
        assert!(!if_none_match_matches(
            &HeaderValue::from_static("W/\"other\""),
            "W/\"current\""
        ));
    }
}
