// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use tower::{Layer, Service, ServiceExt};

/// Defers inner-service readiness until each request is actually forwarded.
///
/// This adapter is useful beneath request-aware middleware that may synthesize
/// a response after its own `poll_ready` call. It prevents that middleware from
/// reserving inner capacity for a request it later rejects.
///
/// This adapter intentionally hides inner backpressure: readiness is awaited in
/// the call future. Put a separately bounded [`crate::service::AdmissionLayer`] or
/// concurrency layer outside it. An outer load-shed layer alone cannot observe
/// inner saturation.
#[derive(Debug, Clone, Copy, Default)]
pub struct OneshotLayer;

impl<S> Layer<S> for OneshotLayer {
    type Service = OneshotService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OneshotService::new(inner)
    }
}

/// Service produced by [`OneshotLayer`].
#[derive(Debug, Clone)]
pub struct OneshotService<S> {
    inner: S,
}

impl<S> OneshotService<S> {
    #[must_use]
    pub fn new(inner: S) -> Self {
        Self { inner }
    }
}

impl<S, Request> Service<Request> for OneshotService<S>
where
    S: Service<Request> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Response: Send + 'static,
    S::Error: Send + 'static,
    Request: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request) -> Self::Future {
        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);
        Box::pin(inner.oneshot(request))
    }
}
