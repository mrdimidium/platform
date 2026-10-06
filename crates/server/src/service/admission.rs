// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    future::Future,
    num::NonZeroUsize,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use http::{Request, Response, StatusCode};
use tokio::sync::Semaphore;
use tower::{Layer, Service, ServiceExt};

/// Rejects work when the configured concurrency budget cannot be acquired.
///
/// The permit is held until the inner service produces a response. Streaming a
/// response body is intentionally outside this budget and should be constrained
/// separately by transport and body policies.
#[derive(Debug, Clone)]
pub struct AdmissionLayer {
    semaphore: Arc<Semaphore>,
    wait: Option<Duration>,
    waiters: Option<Arc<Semaphore>>,
    rejection_status: StatusCode,
}

impl AdmissionLayer {
    #[must_use]
    pub fn new(max_concurrency: NonZeroUsize) -> Self {
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrency.get())),
            wait: None,
            waiters: None,
            rejection_status: StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    /// Waits at most `wait` for capacity with at most `max_waiters` queued calls.
    #[must_use]
    pub fn with_wait(mut self, wait: Duration, max_waiters: NonZeroUsize) -> Self {
        self.wait = Some(wait);
        self.waiters = Some(Arc::new(Semaphore::new(max_waiters.get())));
        self
    }

    #[must_use]
    pub const fn with_rejection_status(mut self, status: StatusCode) -> Self {
        self.rejection_status = status;
        self
    }

    #[must_use]
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }
}

impl<S> Layer<S> for AdmissionLayer {
    type Service = AdmissionService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        AdmissionService {
            inner,
            semaphore: Arc::clone(&self.semaphore),
            wait: self.wait,
            waiters: self.waiters.clone(),
            rejection_status: self.rejection_status,
        }
    }
}

/// Service produced by [`AdmissionLayer`].
#[derive(Debug, Clone)]
pub struct AdmissionService<S> {
    inner: S,
    semaphore: Arc<Semaphore>,
    wait: Option<Duration>,
    waiters: Option<Arc<Semaphore>>,
    rejection_status: StatusCode,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for AdmissionService<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    RequestBody: Send + 'static,
    ResponseBody: Default + Send + 'static,
{
    type Response = Response<ResponseBody>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<RequestBody>) -> Self::Future {
        let semaphore = Arc::clone(&self.semaphore);
        let wait = self.wait;
        let waiters = self.waiters.clone();
        let rejection_status = self.rejection_status;
        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);

        Box::pin(async move {
            let permit = if let Ok(permit) = Arc::clone(&semaphore).try_acquire_owned() {
                Some(permit)
            } else {
                let waiting_permit = waiters.and_then(|waiters| waiters.try_acquire_owned().ok());
                if let (Some(wait), Some(waiting_permit)) = (wait, waiting_permit) {
                    let permit = tokio::time::timeout(wait, semaphore.acquire_owned())
                        .await
                        .ok()
                        .and_then(Result::ok);
                    drop(waiting_permit);
                    permit
                } else {
                    None
                }
            };

            let Some(permit) = permit else {
                return Ok(empty_response(rejection_status));
            };

            let response = inner.oneshot(request).await;
            drop(permit);
            response
        })
    }
}

fn empty_response<B: Default>(status: StatusCode) -> Response<B> {
    let mut response = Response::new(B::default());
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, sync::Arc};

    use http::Request;
    use tokio::sync::Barrier;
    use tower::{ServiceBuilder, ServiceExt, service_fn};

    use super::*;

    #[tokio::test]
    async fn rejects_when_budget_is_exhausted() {
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let service = service_fn({
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
        });
        let service = ServiceBuilder::new()
            .layer(AdmissionLayer::new(NonZeroUsize::new(1).unwrap()))
            .service(service);

        let first = tokio::spawn(service.clone().oneshot(Request::new(())));
        entered.wait().await;

        let rejected = service.clone().oneshot(Request::new(())).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::SERVICE_UNAVAILABLE);

        release.wait().await;
        assert!(first.await.unwrap().unwrap().status().is_success());
    }

    #[tokio::test(start_paused = true)]
    async fn bounded_wait_does_not_create_an_unbounded_queue() {
        let permit = Arc::new(Semaphore::new(0));
        let layer = AdmissionLayer {
            semaphore: permit,
            wait: Some(Duration::from_secs(2)),
            waiters: Some(Arc::new(Semaphore::new(1))),
            rejection_status: StatusCode::TOO_MANY_REQUESTS,
        };
        let service = layer.layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(()))
        }));

        let waiting = tokio::spawn(service.clone().oneshot(Request::new(())));
        tokio::task::yield_now().await;
        let overflow = service.oneshot(Request::new(())).await.unwrap();
        assert_eq!(overflow.status(), StatusCode::TOO_MANY_REQUESTS);

        tokio::time::advance(Duration::from_secs(2)).await;
        let response = waiting.await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    }
}
