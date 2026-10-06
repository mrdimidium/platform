// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{error::Error, fmt, net::IpAddr, num::NonZeroU32, sync::Arc, time::Duration};

use governor::{
    clock::QuantaInstant,
    middleware::{NoOpMiddleware, RateLimitingMiddleware},
};
use http::{Request, Response};
use tower::Layer;
use tower_governor::governor::{Governor, GovernorConfigBuilder};
pub use tower_governor::{GovernorError, key_extractor::KeyExtractor};
pub use tower_governor::{
    governor::GovernorConfig as RateLimitConfig, key_extractor::GlobalKeyExtractor,
};

use super::{ClientIp, OneshotService};

/// Readiness-safe Tower layer for a governor rate limiter.
///
/// Unlike the upstream layer, this adapter does not reserve inner-service
/// capacity for requests that the limiter rejects.
pub struct RateLimitLayer<K, M, ResponseBody>
where
    K: KeyExtractor,
    M: RateLimitingMiddleware<QuantaInstant>,
{
    inner: tower_governor::GovernorLayer<K, M, ResponseBody>,
}

impl<K, M, ResponseBody> RateLimitLayer<K, M, ResponseBody>
where
    K: KeyExtractor,
    M: RateLimitingMiddleware<QuantaInstant>,
{
    #[must_use]
    pub fn new(config: impl Into<Arc<RateLimitConfig<K, M>>>) -> Self {
        Self {
            inner: tower_governor::GovernorLayer::new(config),
        }
    }

    #[must_use]
    pub fn error_handler(
        self,
        handler: impl Fn(GovernorError) -> Response<ResponseBody> + Send + Sync + 'static,
    ) -> Self {
        Self {
            inner: self.inner.error_handler(handler),
        }
    }
}

impl<K, M, ResponseBody> Clone for RateLimitLayer<K, M, ResponseBody>
where
    K: KeyExtractor,
    M: RateLimitingMiddleware<QuantaInstant>,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<K, M, S, ResponseBody> Layer<S> for RateLimitLayer<K, M, ResponseBody>
where
    K: KeyExtractor,
    M: RateLimitingMiddleware<QuantaInstant>,
{
    type Service = RateLimitService<K, M, S, ResponseBody>;

    fn layer(&self, inner: S) -> Self::Service {
        self.inner.layer(OneshotService::new(inner))
    }
}

/// Service produced by [`RateLimitLayer`].
pub type RateLimitService<K, M, S, ResponseBody> = Governor<K, M, OneshotService<S>, ResponseBody>;

/// Builds a keyed rate limiter exclusively from application-supplied values.
///
/// Call `config.limiter().retain_recent()` periodically when the key space is
/// unbounded so idle entries do not accumulate indefinitely.
///
/// # Errors
/// Returns [`RateLimitPolicyError`] when `period` is zero or the underlying
/// limiter rejects the supplied quota.
pub fn rate_limit<K>(
    period: Duration,
    burst: NonZeroU32,
    key_extractor: K,
) -> Result<RateLimitConfig<K, NoOpMiddleware<QuantaInstant>>, RateLimitPolicyError>
where
    K: KeyExtractor,
{
    if period.is_zero() {
        return Err(RateLimitPolicyError::ZeroPeriod);
    }

    let mut builder = GovernorConfigBuilder::default();
    builder.period(period).burst_size(burst.get());
    builder
        .key_extractor(key_extractor)
        .finish()
        .ok_or(RateLimitPolicyError::InvalidQuota)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitPolicyError {
    ZeroPeriod,
    InvalidQuota,
}

impl fmt::Display for RateLimitPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroPeriod => formatter.write_str("rate-limit period must not be zero"),
            Self::InvalidQuota => formatter.write_str("rate-limit quota is invalid"),
        }
    }
}

impl Error for RateLimitPolicyError {}

/// Uses the trusted [`ClientIp`] request extension as a rate-limit key.
///
/// Place [`crate::service::ClientIpLayer`] outside [`RateLimitLayer`]. This extractor
/// deliberately never reads forwarding headers itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientIpKeyExtractor;

impl KeyExtractor for ClientIpKeyExtractor {
    type Key = IpAddr;

    fn extract<Body>(&self, request: &Request<Body>) -> Result<Self::Key, GovernorError> {
        request
            .extensions()
            .get::<ClientIp>()
            .map(|client| client.0)
            .ok_or(GovernorError::UnableToExtractKey)
    }
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, future::poll_fn, sync::Arc};

    use axum::body::Body;
    use http::{Response, StatusCode};
    use tower::{Layer, Service, ServiceExt, limit::ConcurrencyLimitLayer, service_fn};

    use super::*;

    #[tokio::test]
    async fn keys_rate_limits_only_from_authenticated_extension() {
        let config = rate_limit(
            Duration::from_mins(1),
            NonZeroU32::new(1).unwrap(),
            ClientIpKeyExtractor,
        )
        .unwrap();
        let service =
            RateLimitLayer::new(Arc::new(config)).layer(service_fn(|_: Request<Body>| async {
                Ok::<_, Infallible>(Response::new(Body::empty()))
            }));

        let missing = service
            .clone()
            .oneshot(Request::new(Body::empty()))
            .await
            .unwrap();
        assert_eq!(missing.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let mut first = Request::new(Body::empty());
        first
            .extensions_mut()
            .insert(ClientIp("192.0.2.1".parse().unwrap()));
        assert!(
            service
                .clone()
                .oneshot(first)
                .await
                .unwrap()
                .status()
                .is_success()
        );

        let mut second = Request::new(Body::empty());
        second
            .extensions_mut()
            .insert(ClientIp("192.0.2.1".parse().unwrap()));
        let limited = service.oneshot(second).await.unwrap();
        assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(limited.headers().contains_key("retry-after"));
    }

    #[tokio::test]
    async fn rejected_request_does_not_reserve_inner_readiness() {
        let config = rate_limit(
            Duration::from_mins(1),
            NonZeroU32::new(1).unwrap(),
            ClientIpKeyExtractor,
        )
        .unwrap();
        let inner = ConcurrencyLimitLayer::new(1).layer(service_fn(|_: Request<Body>| async {
            Ok::<_, Infallible>(Response::new(Body::empty()))
        }));
        let service = RateLimitLayer::new(Arc::new(config)).layer(inner);
        let mut rejecting_connection = service.clone();
        poll_fn(|context| rejecting_connection.poll_ready(context))
            .await
            .unwrap();
        let rejected = rejecting_connection
            .call(Request::new(Body::empty()))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let mut allowed = Request::new(Body::empty());
        allowed
            .extensions_mut()
            .insert(ClientIp("192.0.2.2".parse().unwrap()));
        let response = tokio::time::timeout(Duration::from_millis(100), service.oneshot(allowed))
            .await
            .expect("rejection must not park the shared concurrency permit")
            .unwrap();
        assert!(response.status().is_success());
    }

    #[test]
    fn rejects_zero_period_without_exposing_dependency_defaults() {
        assert!(matches!(
            rate_limit(
                Duration::ZERO,
                NonZeroU32::new(1).unwrap(),
                GlobalKeyExtractor,
            ),
            Err(RateLimitPolicyError::ZeroPeriod)
        ));
    }
}
