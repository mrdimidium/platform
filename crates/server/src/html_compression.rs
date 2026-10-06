// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use http::{Response, header};
use http_body::Body;
use tower_http::compression::predicate::Predicate;

/// Compression predicate restricted to HTML responses at or above a caller-selected size.
///
/// Use this with `tower_http`'s streaming [`CompressionLayer`]. Unknown body sizes are accepted;
/// the compressor wraps the body and never collects it.
///
/// [`CompressionLayer`]: tower_http::compression::CompressionLayer
#[derive(Debug, Clone, Copy)]
pub struct HtmlCompressionPredicate {
    minimum_size: u16,
}

impl HtmlCompressionPredicate {
    #[must_use]
    pub const fn new(minimum_size: u16) -> Self {
        Self { minimum_size }
    }
}

impl Predicate for HtmlCompressionPredicate {
    fn should_compress<B>(&self, response: &Response<B>) -> bool
    where
        B: Body,
    {
        let is_html = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .is_some_and(|value| value.trim().eq_ignore_ascii_case("text/html"));
        if !is_html {
            return false;
        }

        // Axum strips a HEAD body before an outer layer sees it but preserves the GET length.
        // Prefer that header so HEAD advertises the same negotiated representation as GET.
        let size = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse::<u64>().ok())
            .or_else(|| response.body().size_hint().exact());
        size.is_none_or(|size| size >= u64::from(self.minimum_size))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        pin::Pin,
        task::{Context, Poll},
    };

    use axum::body::{Body as AxumBody, Bytes};
    use http::{HeaderValue, Request, Response};
    use tower::{ServiceBuilder, ServiceExt as _, service_fn};
    use tower_http::compression::CompressionLayer;

    use super::*;

    struct PendingBody;

    impl Body for PendingBody {
        type Data = Bytes;
        type Error = Infallible;

        fn poll_frame(
            self: Pin<&mut Self>,
            _context: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            Poll::Pending
        }
    }

    #[test]
    fn accepts_only_html_at_or_above_the_threshold() {
        let predicate = HtmlCompressionPredicate::new(128);
        assert!(
            !predicate.should_compress(
                &Response::builder()
                    .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .body(AxumBody::from(vec![0; 127]))
                    .unwrap()
            )
        );
        assert!(
            predicate.should_compress(
                &Response::builder()
                    .header(header::CONTENT_TYPE, "TEXT/HTML")
                    .body(AxumBody::from(vec![0; 128]))
                    .unwrap()
            )
        );
        assert!(
            !predicate.should_compress(
                &Response::builder()
                    .header(header::CONTENT_TYPE, "application/octet-stream")
                    .body(AxumBody::from(vec![0; 1_024]))
                    .unwrap()
            )
        );
        assert!(
            predicate.should_compress(
                &Response::builder()
                    .header(header::CONTENT_TYPE, "text/html")
                    .header(header::CONTENT_LENGTH, "128")
                    .body(AxumBody::empty())
                    .unwrap()
            )
        );
    }

    async fn selected_content_encoding(accept_encoding: &'static str) -> Option<HeaderValue> {
        let service = ServiceBuilder::new()
            .layer(
                CompressionLayer::new()
                    .gzip(true)
                    .br(true)
                    .compress_when(HtmlCompressionPredicate::new(128)),
            )
            .service(service_fn(|_: Request<AxumBody>| async {
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                        .body(AxumBody::from(vec![0; 1_024]))
                        .unwrap(),
                )
            }));

        service
            .oneshot(
                Request::builder()
                    .header(header::ACCEPT_ENCODING, accept_encoding)
                    .body(AxumBody::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
            .headers()
            .get(header::CONTENT_ENCODING)
            .cloned()
    }

    #[tokio::test]
    async fn tower_compression_honors_quality_zero_and_wildcard_identity_preference() {
        assert_eq!(
            selected_content_encoding("gzip;q=0.3, br;q=0.8").await,
            Some(HeaderValue::from_static("br"))
        );
        assert_eq!(selected_content_encoding("gzip;q=0, br;q=0").await, None);
        assert_eq!(selected_content_encoding("*;q=0").await, None);
        assert_eq!(selected_content_encoding("*;q=0.5").await, None);
    }

    #[tokio::test]
    async fn tower_compression_returns_before_the_source_body_finishes() {
        let service = ServiceBuilder::new()
            .layer(
                CompressionLayer::new()
                    .gzip(true)
                    .br(false)
                    .compress_when(HtmlCompressionPredicate::new(128)),
            )
            .service(service_fn(|_: Request<AxumBody>| async {
                Ok::<_, Infallible>(
                    Response::builder()
                        .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                        .header(header::CONTENT_LENGTH, "1024")
                        .body(AxumBody::new(PendingBody))
                        .unwrap(),
                )
            }));

        let response = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            service.oneshot(
                Request::builder()
                    .header(header::ACCEPT_ENCODING, "gzip")
                    .body(AxumBody::empty())
                    .unwrap(),
            ),
        )
        .await
        .expect("streaming compression must not wait for end of body")
        .unwrap();
        assert_eq!(response.headers()[header::CONTENT_ENCODING], "gzip");
        assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
    }
}
