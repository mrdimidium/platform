// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use http::{
    Request, Response, StatusCode, header,
    uri::{Authority, InvalidUri},
};
use tower::{Layer, Service, ServiceExt};

/// Rejects requests whose authority is not in an application-supplied allowlist.
#[derive(Debug, Clone)]
pub struct HostLayer {
    allowed: Vec<HostPattern>,
    rejection_status: StatusCode,
}

impl HostLayer {
    #[must_use]
    pub fn new(allowed: impl IntoIterator<Item = HostPattern>) -> Self {
        Self {
            allowed: allowed.into_iter().collect(),
            rejection_status: StatusCode::MISDIRECTED_REQUEST,
        }
    }

    #[must_use]
    pub const fn with_rejection_status(mut self, status: StatusCode) -> Self {
        self.rejection_status = status;
        self
    }
}

impl<S> Layer<S> for HostLayer {
    type Service = HostService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        HostService {
            inner,
            allowed: self.allowed.clone(),
            rejection_status: self.rejection_status,
        }
    }
}

/// Service produced by [`HostLayer`].
#[derive(Debug, Clone)]
pub struct HostService<S> {
    inner: S,
    allowed: Vec<HostPattern>,
    rejection_status: StatusCode,
}

impl<S, RequestBody, ResponseBody> Service<Request<RequestBody>> for HostService<S>
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
        let status = match validate_authority(&request, &self.allowed) {
            Ok(()) => StatusCode::OK,
            Err(StatusCode::MISDIRECTED_REQUEST) => self.rejection_status,
            Err(status) => status,
        };
        if status != StatusCode::OK {
            return Box::pin(async move { Ok(empty_response(status)) });
        }

        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);
        Box::pin(async move { inner.oneshot(request).await })
    }
}

/// A normalized host and optional port accepted by [`HostLayer`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct HostPattern {
    host: String,
    port: Option<u16>,
}

impl HostPattern {
    /// Creates a host rule. A rule without a port accepts that host on any port.
    ///
    /// # Errors
    /// Returns [`InvalidUri`] when `authority` is not a valid HTTP authority.
    pub fn new(authority: &str) -> Result<Self, InvalidUri> {
        let authority = authority.parse::<Authority>()?;
        Ok(Self {
            host: authority.host().to_ascii_lowercase(),
            port: authority.port_u16(),
        })
    }

    pub(crate) fn matches(&self, authority: &Authority) -> bool {
        self.host.eq_ignore_ascii_case(authority.host())
            && self
                .port
                .is_none_or(|port| authority.port_u16() == Some(port))
    }

    pub(crate) fn matches_uri(&self, uri: &http::Uri) -> bool {
        let Some(host) = uri.host() else {
            return false;
        };
        let port = uri.port_u16().or_else(|| match uri.scheme_str() {
            Some("http") => Some(80),
            Some("https") => Some(443),
            _ => None,
        });
        self.host.eq_ignore_ascii_case(host)
            && self.port.is_none_or(|expected| port == Some(expected))
    }
}

fn validate_authority<B>(request: &Request<B>, allowed: &[HostPattern]) -> Result<(), StatusCode> {
    let mut host_headers = request.headers().get_all(header::HOST).iter();
    let header_authority = host_headers
        .next()
        .map(|value| {
            value
                .to_str()
                .ok()
                .and_then(|value| value.parse::<Authority>().ok())
                .ok_or(StatusCode::BAD_REQUEST)
        })
        .transpose()?;
    if host_headers.next().is_some() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let uri_authority = request.uri().authority();

    if let (Some(header), Some(uri)) = (&header_authority, uri_authority)
        && !same_authority(header, uri)
    {
        return Err(StatusCode::BAD_REQUEST);
    }

    let authority = uri_authority
        .or(header_authority.as_ref())
        .ok_or(StatusCode::BAD_REQUEST)?;
    if allowed.iter().any(|pattern| pattern.matches(authority)) {
        Ok(())
    } else {
        Err(StatusCode::MISDIRECTED_REQUEST)
    }
}

fn same_authority(left: &Authority, right: &Authority) -> bool {
    left.host().eq_ignore_ascii_case(right.host()) && left.port_u16() == right.port_u16()
}

fn empty_response<B: Default>(status: StatusCode) -> Response<B> {
    let mut response = Response::new(B::default());
    *response.status_mut() = status;
    response
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, future::poll_fn, time::Duration};

    use tower::{
        Layer, Service, ServiceBuilder, ServiceExt, limit::ConcurrencyLimitLayer, service_fn,
    };

    use super::*;

    fn service() -> impl Service<Request<()>, Response = Response<()>, Error = Infallible> + Clone {
        ServiceBuilder::new()
            .layer(HostLayer::new([
                HostPattern::new("example.test").unwrap(),
                HostPattern::new("admin.example.test:8443").unwrap(),
            ]))
            .service(service_fn(|_: Request<()>| async {
                Ok::<_, Infallible>(Response::new(()))
            }))
    }

    #[tokio::test]
    async fn accepts_allowed_host_and_optional_port() {
        for host in ["example.test", "example.test:8080", "EXAMPLE.TEST"] {
            let request = Request::builder()
                .header(header::HOST, host)
                .body(())
                .unwrap();
            assert!(
                service()
                    .oneshot(request)
                    .await
                    .unwrap()
                    .status()
                    .is_success()
            );
        }
    }

    #[tokio::test]
    async fn rejection_does_not_reserve_inner_readiness() {
        let inner = ConcurrencyLimitLayer::new(1).layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(()))
        }));
        let service = HostLayer::new([HostPattern::new("example.test").unwrap()]).layer(inner);
        let mut rejecting_connection = service.clone();
        poll_fn(|context| rejecting_connection.poll_ready(context))
            .await
            .unwrap();
        let rejected = rejecting_connection
            .call(
                Request::builder()
                    .header(header::HOST, "attacker.test")
                    .body(())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::MISDIRECTED_REQUEST);

        let allowed = Request::builder()
            .header(header::HOST, "example.test")
            .body(())
            .unwrap();
        tokio::time::timeout(Duration::from_millis(100), service.oneshot(allowed))
            .await
            .expect("rejection must not park the shared concurrency permit")
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_unknown_missing_invalid_and_conflicting_hosts() {
        let unknown = Request::builder()
            .header(header::HOST, "attacker.test")
            .body(())
            .unwrap();
        assert_eq!(
            service().oneshot(unknown).await.unwrap().status(),
            StatusCode::MISDIRECTED_REQUEST
        );

        assert_eq!(
            service().oneshot(Request::new(())).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );

        let conflict = Request::builder()
            .uri("https://example.test/")
            .header(header::HOST, "attacker.test")
            .body(())
            .unwrap();
        assert_eq!(
            service().oneshot(conflict).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );

        let wrong_port = Request::builder()
            .header(header::HOST, "admin.example.test:443")
            .body(())
            .unwrap();
        assert_eq!(
            service().oneshot(wrong_port).await.unwrap().status(),
            StatusCode::MISDIRECTED_REQUEST
        );

        let mut duplicate = Request::builder()
            .header(header::HOST, "example.test")
            .body(())
            .unwrap();
        duplicate
            .headers_mut()
            .append(header::HOST, "attacker.test".parse().unwrap());
        assert_eq!(
            service().oneshot(duplicate).await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
}
