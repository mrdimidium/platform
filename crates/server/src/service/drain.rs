// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::Duration,
};

use http::{Request, Response, StatusCode};
use http_body::Body;
use pin_project_lite::pin_project;
use tokio::sync::Notify;
use tower::{Layer, Service, ServiceExt};

/// Rejects new requests after draining starts and tracks streaming responses.
#[derive(Debug, Clone)]
pub struct DrainLayer {
    state: Arc<State>,
    rejection_status: StatusCode,
}

impl DrainLayer {
    #[must_use]
    pub fn new() -> (Self, DrainHandle) {
        let state = Arc::new(State {
            accepting: AtomicBool::new(true),
            active: AtomicUsize::new(0),
            idle: Notify::new(),
        });
        (
            Self {
                state: Arc::clone(&state),
                rejection_status: StatusCode::SERVICE_UNAVAILABLE,
            },
            DrainHandle { state },
        )
    }

    #[must_use]
    pub const fn with_rejection_status(mut self, status: StatusCode) -> Self {
        self.rejection_status = status;
        self
    }
}

impl<S> Layer<S> for DrainLayer {
    type Service = DrainService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        DrainService {
            inner,
            state: Arc::clone(&self.state),
            rejection_status: self.rejection_status,
        }
    }
}

/// Service produced by [`DrainLayer`].
#[derive(Debug, Clone)]
pub struct DrainService<S> {
    inner: S,
    state: Arc<State>,
    rejection_status: StatusCode,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for DrainService<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    RequestBody: Send + 'static,
    ResponseBody: Default + Send + 'static,
{
    type Response = Response<DrainBody<ResponseBody>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<RequestBody>) -> Self::Future {
        if !try_enter(&self.state) {
            let response = empty_response(self.rejection_status);
            return Box::pin(async move { Ok(response) });
        }

        let guard = ActiveGuard {
            state: Arc::clone(&self.state),
        };
        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);
        Box::pin(async move {
            inner
                .oneshot(request)
                .await
                .map(|response| response.map(|body| DrainBody::new(body, guard)))
        })
    }
}

#[derive(Debug)]
struct State {
    accepting: AtomicBool,
    active: AtomicUsize,
    idle: Notify,
}

/// Starts request draining and waits for requests admitted by [`DrainLayer`],
/// including response-body streaming.
#[derive(Debug, Clone)]
pub struct DrainHandle {
    state: Arc<State>,
}

impl DrainHandle {
    /// Rejects future requests. Returns `true` only for the first caller.
    #[must_use]
    pub fn begin(&self) -> bool {
        self.state.accepting.swap(false, Ordering::SeqCst)
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        !self.state.accepting.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn active(&self) -> usize {
        self.state.active.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) {
        loop {
            let idle = self.state.idle.notified();
            if self.active() == 0 {
                return;
            }
            idle.await;
        }
    }

    /// Returns `true` when all admitted handlers finish before `timeout`.
    pub async fn wait_for(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.wait()).await.is_ok()
    }
}

fn try_enter(state: &Arc<State>) -> bool {
    if !state.accepting.load(Ordering::SeqCst) {
        return false;
    }
    state.active.fetch_add(1, Ordering::SeqCst);
    if state.accepting.load(Ordering::SeqCst) {
        true
    } else {
        leave(state);
        false
    }
}

fn leave(state: &State) {
    if state.active.fetch_sub(1, Ordering::SeqCst) == 1 {
        state.idle.notify_waiters();
    }
}

struct ActiveGuard {
    state: Arc<State>,
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        leave(&self.state);
    }
}

pin_project! {
    /// Response body retaining a drain permit until the body ends or is dropped.
    pub struct DrainBody<B> {
        #[pin]
        inner: B,
        guard: Option<ActiveGuard>,
    }
}

impl<B> DrainBody<B> {
    fn new(inner: B, guard: ActiveGuard) -> Self {
        Self {
            inner,
            guard: Some(guard),
        }
    }

    fn rejected(inner: B) -> Self {
        Self { inner, guard: None }
    }
}

impl<B> Body for DrainBody<B>
where
    B: Body,
{
    type Data = B::Data;
    type Error = B::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        let frame = this.inner.as_mut().poll_frame(context);
        if matches!(frame, Poll::Ready(None)) {
            this.guard.take();
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.inner.size_hint()
    }
}

fn empty_response<B: Default>(status: StatusCode) -> Response<DrainBody<B>> {
    let mut response = Response::new(DrainBody::rejected(B::default()));
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, sync::Arc};

    use axum::{
        Router,
        body::{Body as AxumBody, Bytes},
        routing::get,
    };
    use http_body_util::{BodyExt, Full};
    use tokio::sync::Barrier;
    use tower::{ServiceExt, service_fn};

    use super::*;

    #[tokio::test]
    async fn composes_with_an_axum_router() {
        let (layer, handle) = DrainLayer::new();
        let router = Router::new()
            .route("/", get(|| async { "ok" }))
            .layer(layer);
        let response = router
            .oneshot(Request::new(AxumBody::empty()))
            .await
            .unwrap();
        assert_eq!(handle.active(), 1);
        assert_eq!(
            response.into_body().collect().await.unwrap().to_bytes(),
            "ok"
        );
        assert_eq!(handle.active(), 0);
    }

    #[tokio::test]
    async fn keeps_request_active_until_the_response_body_finishes() {
        let (layer, handle) = DrainLayer::new();
        let service = layer.layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::from_static(b"body"))))
        }));

        let response = service.oneshot(Request::new(())).await.unwrap();
        assert_eq!(handle.active(), 1);
        response.into_body().collect().await.unwrap();
        assert_eq!(handle.active(), 0);
    }

    #[tokio::test]
    async fn rejects_new_requests_and_waits_for_admitted_handlers() {
        let (layer, handle) = DrainLayer::new();
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let service = layer.layer(service_fn({
            let entered = Arc::clone(&entered);
            let release = Arc::clone(&release);
            move |_: Request<()>| {
                let entered = Arc::clone(&entered);
                let release = Arc::clone(&release);
                async move {
                    entered.wait().await;
                    release.wait().await;
                    Ok::<_, Infallible>(Response::new(()))
                }
            }
        }));

        let first = tokio::spawn(service.clone().oneshot(Request::new(())));
        entered.wait().await;
        assert_eq!(handle.active(), 1);
        assert!(handle.begin());
        assert!(!handle.begin());

        let rejected = service.oneshot(Request::new(())).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!handle.wait_for(Duration::ZERO).await);

        release.wait().await;
        first.await.unwrap().unwrap();
        handle.wait().await;
        assert_eq!(handle.active(), 0);
    }
}
