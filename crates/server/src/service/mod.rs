// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

//! Small, independent Tower services for HTTP server hardening.
//!
//! This module deliberately contains no configuration loading, application
//! defaults, authentication policy, or observability. Applications supply all
//! concrete limits and compose only the layers appropriate for each route.

pub mod admission;
pub mod assets;
pub mod client_ip;
pub mod deadline_body;
pub mod drain;
pub mod host;
pub mod hsts;
pub mod html;
pub mod oneshot;
pub mod rate_limit;
pub mod response_body;

/// Incoming request-body size limiting primitives.
pub mod body {
    pub use tower_http::limit::{RequestBodyLimit, RequestBodyLimitLayer};
}

/// Whole-request and streaming-body timeout primitives.
pub mod timeout {
    pub use tower_http::timeout::{
        RequestBodyTimeout, RequestBodyTimeoutLayer, ResponseBodyTimeout, ResponseBodyTimeoutLayer,
        Timeout, TimeoutBody, TimeoutError, TimeoutLayer,
    };
}

/// Response compression and request decompression primitives.
///
/// To bound compression amplification, apply one request-body limit outside
/// decompression and a second request-body limit inside it. The application
/// supplies both the encoded and decoded size budgets.
pub mod compression {
    pub use tower_http::{
        compression::{Compression, CompressionLayer, CompressionLevel, Predicate},
        decompression::{RequestDecompression, RequestDecompressionLayer},
    };
}

pub mod redirect;

/// Error-based limits for outbound Tower clients.
///
/// Combine these with [`OutboundUriLayer`], [`ResponseBodyLimitLayer`],
/// [`ResponseBodyDeadlineLayer`], and `timeout::ResponseBodyTimeoutLayer`.
pub mod outbound {
    pub use tower::{
        limit::{ConcurrencyLimit, ConcurrencyLimitLayer},
        load_shed::{LoadShed, LoadShedLayer},
        timeout::{Timeout, TimeoutLayer},
    };
}

pub use admission::{AdmissionLayer, AdmissionService};
pub use assets::{AssetsLayer, AssetsService};
pub use client_ip::{
    ClientIp, ClientIpLayer, ClientIpService, ForwardedHeader, PeerAddr, TrustedProxies,
};
pub use deadline_body::{
    DeadlineBody, DeadlineError, ResponseBodyDeadlineLayer, ResponseBodyDeadlineService,
};
pub use drain::{DrainBody, DrainHandle, DrainLayer, DrainService};
pub use host::{HostLayer, HostPattern, HostService};
pub use hsts::{HstsFuture, HstsLayer, HstsService};
pub use html::{
    DEFAULT_MAX_HTML_BODY_BYTES, HtmlLayer, HtmlService, POLICY_CSP, POLICY_PERMISSIONS,
    content_security_policy_for_scripts,
};
pub use oneshot::{OneshotLayer, OneshotService};
pub use rate_limit::{
    ClientIpKeyExtractor, GlobalKeyExtractor, GovernorError, KeyExtractor, RateLimitConfig,
    RateLimitLayer, RateLimitPolicyError, RateLimitService, rate_limit,
};
pub use redirect::{
    AllowedRedirects, OutboundUriLayer, OutboundUriService, SafeRedirects, UriRejected,
};
pub use response_body::{ResponseBodyLimitLayer, ResponseBodyLimitService};
