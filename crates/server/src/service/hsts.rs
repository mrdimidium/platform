// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    future::Future,
    task::{Context, Poll, ready},
};

use http::{HeaderValue, Request, Response, header};
use tower::{Layer, Service};

/// Adds Strict-Transport-Security to every response from an HTTPS-only listener.
///
/// Applications supply the complete policy value and must not install this
/// layer on plaintext listeners.
#[derive(Debug, Clone)]
pub struct HstsLayer {
    value: HeaderValue,
}

impl HstsLayer {
    #[must_use]
    pub const fn new(value: HeaderValue) -> Self {
        Self { value }
    }
}

impl<S> Layer<S> for HstsLayer {
    type Service = HstsService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HstsService {
            inner,
            value: self.value.clone(),
        }
    }
}

/// Service produced by [`HstsLayer`].
#[derive(Debug, Clone)]
pub struct HstsService<S> {
    inner: S,
    value: HeaderValue,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for HstsService<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody>>,
{
    type Response = Response<ResponseBody>;
    type Error = S::Error;
    type Future = HstsFuture<S::Future>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request<RequestBody>) -> Self::Future {
        HstsFuture {
            inner: self.inner.call(request),
            value: self.value.clone(),
        }
    }
}

pin_project_lite::pin_project! {
    pub struct HstsFuture<F> {
        #[pin]
        inner: F,
        value: HeaderValue,
    }
}

impl<F, ResponseBody, Error> Future for HstsFuture<F>
where
    F: std::future::Future<Output = Result<Response<ResponseBody>, Error>>,
{
    type Output = Result<Response<ResponseBody>, Error>;

    fn poll(self: std::pin::Pin<&mut Self>, context: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let mut response = ready!(this.inner.poll(context))?;
        response
            .headers_mut()
            .insert(header::STRICT_TRANSPORT_SECURITY, this.value.clone());
        Poll::Ready(Ok(response))
    }
}

#[cfg(test)]
mod tests {
    use std::convert::Infallible;

    use http::StatusCode;
    use tower::{Layer, ServiceExt, service_fn};

    use super::*;

    #[tokio::test]
    async fn applies_the_application_policy_to_error_responses() {
        let service = HstsLayer::new(HeaderValue::from_static("max-age=60")).layer(service_fn(
            |_: Request<()>| async {
                let mut response = Response::new(());
                *response.status_mut() = StatusCode::NOT_FOUND;
                Ok::<_, Infallible>(response)
            },
        ));
        let response = service.oneshot(Request::new(())).await.unwrap();
        assert_eq!(
            response.headers()[header::STRICT_TRANSPORT_SECURITY],
            "max-age=60"
        );
    }
}
