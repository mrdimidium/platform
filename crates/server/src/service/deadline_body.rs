// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll, ready},
    time::Duration,
};

use http::{Request, Response};
use http_body::Body;
use pin_project_lite::pin_project;
use tokio::time::{Sleep, sleep};
use tower::{Layer, Service};

/// Enforces a total deadline while a response body is consumed.
#[derive(Debug, Clone, Copy)]
pub struct ResponseBodyDeadlineLayer {
    timeout: Duration,
}

impl ResponseBodyDeadlineLayer {
    #[must_use]
    pub const fn new(timeout: Duration) -> Self {
        Self { timeout }
    }
}

impl<S> Layer<S> for ResponseBodyDeadlineLayer {
    type Service = ResponseBodyDeadlineService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ResponseBodyDeadlineService {
            inner,
            timeout: self.timeout,
        }
    }
}

/// Service produced by [`ResponseBodyDeadlineLayer`].
#[derive(Debug, Clone)]
pub struct ResponseBodyDeadlineService<S> {
    inner: S,
    timeout: Duration,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for ResponseBodyDeadlineService<S>
where
    S: Service<Request<RequestBody>, Response = Response<ResponseBody>>,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
    ResponseBody: Send + 'static,
{
    type Response = Response<DeadlineBody<ResponseBody>>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, request: Request<RequestBody>) -> Self::Future {
        let timeout = self.timeout;
        let future = self.inner.call(request);
        Box::pin(async move {
            future
                .await
                .map(|response| response.map(|body| DeadlineBody::new(body, timeout)))
        })
    }
}

type BoxError = Box<dyn Error + Send + Sync>;

pin_project! {
    /// A body that must finish before one total deadline.
    ///
    /// Unlike an idle body timeout, this timer is not reset after each frame.
    pub struct DeadlineBody<B> {
        #[pin]
        body: B,

        #[pin]
        deadline: Sleep,

        finished: bool,
    }
}

impl<B> DeadlineBody<B> {
    #[must_use]
    pub fn new(body: B, timeout: Duration) -> Self {
        Self {
            body,
            deadline: sleep(timeout),
            finished: false,
        }
    }
}

impl<B> Body for DeadlineBody<B>
where
    B: Body,
    B::Error: Into<BoxError>,
{
    type Data = B::Data;
    type Error = BoxError;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        if *this.finished {
            return Poll::Ready(None);
        }
        if this.deadline.as_mut().poll(context).is_ready() {
            *this.finished = true;
            return Poll::Ready(Some(Err(Box::new(DeadlineError))));
        }

        let frame = ready!(this.body.as_mut().poll_frame(context));
        if frame.is_none() {
            *this.finished = true;
        }
        Poll::Ready(frame.transpose().map_err(Into::into).transpose())
    }

    fn is_end_stream(&self) -> bool {
        self.finished || self.body.is_end_stream()
    }

    fn size_hint(&self) -> http_body::SizeHint {
        self.body.size_hint()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeadlineError;

impl fmt::Display for DeadlineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("body did not finish before its deadline")
    }
}

impl Error for DeadlineError {}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, task::Poll};

    use axum::body::Bytes;
    use http_body_util::BodyExt;
    use tower::{Layer, ServiceExt, service_fn};

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

    #[tokio::test(start_paused = true)]
    async fn bounds_total_response_consumption_time() {
        let service = ResponseBodyDeadlineLayer::new(Duration::from_secs(2)).layer(service_fn(
            |_: Request<()>| async { Ok::<_, Infallible>(Response::new(PendingBody)) },
        ));
        let response = service.oneshot(Request::new(())).await.unwrap();
        let task = tokio::spawn(response.into_body().collect());
        tokio::time::advance(Duration::from_secs(2)).await;
        let error = task.await.unwrap().unwrap_err();
        assert!(error.downcast_ref::<DeadlineError>().is_some());
    }
}
