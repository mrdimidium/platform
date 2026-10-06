// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use http::{Request, Response};
use http_body_util::Limited;
use tower::{Layer, Service};

/// Enforces a streaming response-body limit without buffering the response.
///
/// This is suitable for wrapping an outbound HTTP client. Exceeding the limit
/// surfaces `http_body_util::LengthLimitError` while the caller consumes the
/// body. Applications may additionally reject an oversized `Content-Length`
/// before beginning consumption.
#[derive(Debug, Clone, Copy)]
pub struct ResponseBodyLimitLayer {
    limit: usize,
}

impl ResponseBodyLimitLayer {
    #[must_use]
    pub const fn new(limit: usize) -> Self {
        Self { limit }
    }
}

impl<S> Layer<S> for ResponseBodyLimitLayer {
    type Service = ResponseBodyLimitService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ResponseBodyLimitService {
            inner,
            limit: self.limit,
        }
    }
}

/// Service produced by [`ResponseBodyLimitLayer`].
#[derive(Debug, Clone)]
pub struct ResponseBodyLimitService<S> {
    inner: S,
    limit: usize,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for ResponseBodyLimitService<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResponseBody: Send + 'static,
{
    type Response = Response<Limited<ResponseBody>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request<RequestBody>) -> Self::Future {
        let limit = self.limit;
        let future = self.inner.call(request);
        Box::pin(async move {
            future
                .await
                .map(|response| response.map(|body| Limited::new(body, limit)))
        })
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use axum::body::Bytes;
    use http_body_util::{BodyExt, Full};
    use tower::{Layer, ServiceExt, service_fn};

    use super::*;

    #[tokio::test]
    async fn limits_streamed_response_without_collecting_in_the_service() {
        let service = ResponseBodyLimitLayer::new(3).layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"four"))))
        }));
        let response = service.oneshot(Request::new(())).await.unwrap();
        assert!(response.into_body().collect().await.is_err());

        let service = ResponseBodyLimitLayer::new(4).layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"four"))))
        }));
        let bytes = service
            .oneshot(Request::new(()))
            .await
            .unwrap()
            .into_body()
            .collect()
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(bytes, "four");
    }
}
