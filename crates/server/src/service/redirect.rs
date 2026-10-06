// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

//! Redirect primitives for outbound Tower HTTP clients.

use std::{
    collections::HashSet,
    error::Error,
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};

use http::{Request, uri::Scheme};
use tower::{Layer, Service, ServiceExt};
pub use tower_http::follow_redirect::policy::{
    self, Action, Attempt, FilterCredentials, Limited, Policy, PolicyExt,
};
pub use tower_http::follow_redirect::{FollowRedirect, FollowRedirectLayer};

use super::HostPattern;

/// Rejects an outbound request before DNS resolution when its URI is outside
/// the allowed scheme/host set.
#[derive(Debug, Clone)]
pub struct OutboundUriLayer {
    policy: AllowedRedirects,
}

impl OutboundUriLayer {
    #[must_use]
    pub const fn new(policy: AllowedRedirects) -> Self {
        Self { policy }
    }
}

impl<S> Layer<S> for OutboundUriLayer {
    type Service = OutboundUriService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        OutboundUriService {
            inner,
            policy: self.policy.clone(),
        }
    }
}

/// Service produced by [`OutboundUriLayer`].
#[derive(Debug, Clone)]
pub struct OutboundUriService<S> {
    inner: S,
    policy: AllowedRedirects,
}

impl<S, Body> Service<Request<Body>> for OutboundUriService<S>
where
    S: Service<Request<Body>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Into<BoxError> + Send + Sync + 'static,
    S::Response: Send + 'static,
    Body: Send + 'static,
{
    type Response = S::Response;
    type Error = BoxError;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, request: Request<Body>) -> Self::Future {
        if !self.policy.allows(request.uri()) {
            return Box::pin(async { Err(Box::new(UriRejected) as BoxError) });
        }
        let replacement = self.inner.clone();
        let inner = std::mem::replace(&mut self.inner, replacement);
        Box::pin(async move { inner.oneshot(request).await.map_err(Into::into) })
    }
}

type BoxError = Box<dyn Error + Send + Sync>;

/// Allows redirects only to explicitly listed destination hosts and schemes.
///
/// Compose this with [`Limited`] and [`FilterCredentials`], or with the
/// standard policy, so redirect count and credential stripping are enforced as
/// independent concerns.
#[derive(Debug, Clone)]
pub struct AllowedRedirects {
    hosts: Vec<HostPattern>,
    schemes: HashSet<Scheme>,
}

impl AllowedRedirects {
    #[must_use]
    pub fn new(
        hosts: impl IntoIterator<Item = HostPattern>,
        schemes: impl IntoIterator<Item = Scheme>,
    ) -> Self {
        Self {
            hosts: hosts.into_iter().collect(),
            schemes: schemes.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn https_only(hosts: impl IntoIterator<Item = HostPattern>) -> Self {
        Self::new(hosts, [Scheme::HTTPS])
    }

    /// Checks an initial or redirected URI against this destination policy.
    #[must_use]
    pub fn allows(&self, location: &http::Uri) -> bool {
        location
            .scheme()
            .is_some_and(|scheme| self.schemes.contains(scheme))
            && self.hosts.iter().any(|host| host.matches_uri(location))
    }
}

impl<Body, Error> Policy<Body, Error> for AllowedRedirects {
    fn redirect(&mut self, attempt: &Attempt<'_>) -> Result<Action, Error> {
        Ok(if self.allows(attempt.location()) {
            Action::Follow
        } else {
            Action::Stop
        })
    }
}

/// A secure-by-construction redirect policy combining a hop limit,
/// destination allowlist, and cross-origin credential stripping.
#[derive(Debug, Clone)]
pub struct SafeRedirects {
    remaining: usize,
    allowed: AllowedRedirects,
    credentials: FilterCredentials,
}

impl SafeRedirects {
    #[must_use]
    pub fn new(max_redirects: usize, allowed: AllowedRedirects) -> Self {
        Self {
            remaining: max_redirects,
            allowed,
            credentials: FilterCredentials::new(),
        }
    }
}

impl<Body, Error> Policy<Body, Error> for SafeRedirects {
    fn redirect(&mut self, attempt: &Attempt<'_>) -> Result<Action, Error> {
        if self.remaining == 0 || !self.allowed.allows(attempt.location()) {
            return Ok(Action::Stop);
        }
        self.remaining -= 1;
        <FilterCredentials as Policy<Body, Error>>::redirect(&mut self.credentials, attempt)
    }

    fn on_request(&mut self, request: &mut Request<Body>) {
        <FilterCredentials as Policy<Body, Error>>::on_request(&mut self.credentials, request);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UriRejected;

impl fmt::Display for UriRejected {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("outbound URI is outside the allowed destination policy")
    }
}

impl Error for UriRejected {}

#[cfg(test)]
mod tests {
    use std::{
        convert::Infallible,
        future::poll_fn,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use axum::body::Body;
    use http::{Response, StatusCode, Uri, header};
    use tower::{Layer, Service, ServiceExt, limit::ConcurrencyLimitLayer, service_fn};

    use super::*;

    #[test]
    fn accepts_only_explicit_https_destinations() {
        let policy =
            AllowedRedirects::https_only([HostPattern::new("downloads.example.test:443").unwrap()]);
        let _safe = SafeRedirects::new(3, policy.clone());
        assert!(policy.allows(&Uri::from_static("https://downloads.example.test/archive")));
        assert!(!policy.allows(&Uri::from_static("http://downloads.example.test/archive")));
        assert!(!policy.allows(&Uri::from_static("https://attacker.example/archive")));
        assert!(!policy.allows(&Uri::from_static("/relative")));
    }

    #[tokio::test]
    async fn safe_policy_limits_hops_and_strips_cross_origin_credentials() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let inner = service_fn({
            let calls = Arc::clone(&calls);
            move |request: Request<Body>| {
                let calls = Arc::clone(&calls);
                async move {
                    calls.lock().unwrap().push((
                        request.uri().clone(),
                        request.headers().contains_key(header::AUTHORIZATION),
                    ));
                    let response = if request.uri().host() == Some("origin.example") {
                        Response::builder()
                            .status(StatusCode::FOUND)
                            .header(header::LOCATION, "https://downloads.example.test/archive")
                            .body(Body::empty())
                            .unwrap()
                    } else {
                        Response::new(Body::empty())
                    };
                    Ok::<_, Infallible>(response)
                }
            }
        });
        let destinations = AllowedRedirects::https_only([
            HostPattern::new("origin.example").unwrap(),
            HostPattern::new("downloads.example.test").unwrap(),
        ]);
        let client = FollowRedirect::with_policy(inner, SafeRedirects::new(1, destinations));
        let request = Request::builder()
            .uri("https://origin.example/start")
            .header(header::AUTHORIZATION, "Bearer secret")
            .body(Body::empty())
            .unwrap();

        assert!(client.oneshot(request).await.unwrap().status().is_success());
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert!(calls[0].1);
        assert!(!calls[1].1);
    }

    #[tokio::test]
    async fn rejected_uri_does_not_reserve_inner_readiness() {
        let inner = ConcurrencyLimitLayer::new(1).layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(()))
        }));
        let service = OutboundUriLayer::new(AllowedRedirects::https_only([HostPattern::new(
            "downloads.example.test",
        )
        .unwrap()]))
        .layer(inner);
        let mut rejecting_connection = service.clone();
        poll_fn(|context| rejecting_connection.poll_ready(context))
            .await
            .unwrap();
        let rejected = rejecting_connection
            .call(
                Request::builder()
                    .uri("https://attacker.example/archive")
                    .body(())
                    .unwrap(),
            )
            .await;
        assert!(
            rejected
                .unwrap_err()
                .downcast_ref::<UriRejected>()
                .is_some()
        );

        let allowed = Request::builder()
            .uri("https://downloads.example.test/archive")
            .body(())
            .unwrap();
        tokio::time::timeout(Duration::from_millis(100), service.oneshot(allowed))
            .await
            .expect("rejection must not park the shared concurrency permit")
            .unwrap();
    }

    #[tokio::test]
    async fn rejects_initial_uri_before_calling_the_client() {
        let service = OutboundUriLayer::new(AllowedRedirects::https_only([HostPattern::new(
            "downloads.example.test",
        )
        .unwrap()]))
        .layer(service_fn(|_: Request<()>| async {
            Ok::<_, Infallible>(Response::new(()))
        }));

        let allowed = Request::builder()
            .uri("https://downloads.example.test/archive")
            .body(())
            .unwrap();
        assert!(service.clone().oneshot(allowed).await.is_ok());

        let denied = Request::builder()
            .uri("https://attacker.example/archive")
            .body(())
            .unwrap();
        let error = service.oneshot(denied).await.unwrap_err();
        assert!(error.downcast_ref::<UriRejected>().is_some());
    }
}
