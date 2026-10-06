// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{error::Error, fmt, num::NonZeroU32, time::Duration};

use hyper_util::{
    rt::{TokioExecutor, TokioTimer},
    server::conn::auto::Builder,
};

const MIN_HTTP1_BUFFER_BYTES: usize = 8 * 1024;

/// Explicit HTTP/1 and HTTP/2 transport limits for Hyper connections.
///
/// Applications supply every value; this type only applies the policy consistently
/// and ensures Hyper's timers are installed.
#[derive(Debug, Clone)]
pub struct HttpTransport {
    header_read_timeout: Duration,
    http1_max_buffer_bytes: usize,
    http2_max_concurrent_streams: NonZeroU32,
    http2_max_header_list_bytes: NonZeroU32,
    http2_keep_alive: Option<(Duration, Duration)>,
}

impl HttpTransport {
    /// Creates a transport policy without application defaults.
    ///
    /// # Errors
    /// Returns [`TransportPolicyError`] for a zero header timeout or an HTTP/1
    /// buffer smaller than Hyper's protocol minimum.
    pub fn new(
        header_read_timeout: Duration,
        http1_max_buffer_bytes: usize,
        http2_max_concurrent_streams: NonZeroU32,
        http2_max_header_list_bytes: NonZeroU32,
    ) -> Result<Self, TransportPolicyError> {
        if header_read_timeout.is_zero() {
            return Err(TransportPolicyError::ZeroDuration("header_read_timeout"));
        }
        if http1_max_buffer_bytes < MIN_HTTP1_BUFFER_BYTES {
            return Err(TransportPolicyError::Http1BufferTooSmall {
                minimum: MIN_HTTP1_BUFFER_BYTES,
                actual: http1_max_buffer_bytes,
            });
        }
        Ok(Self {
            header_read_timeout,
            http1_max_buffer_bytes,
            http2_max_concurrent_streams,
            http2_max_header_list_bytes,
            http2_keep_alive: None,
        })
    }

    /// Enables HTTP/2 keep-alive probes.
    ///
    /// # Errors
    /// Returns [`TransportPolicyError`] when either duration is zero.
    pub fn with_http2_keep_alive(
        mut self,
        interval: Duration,
        timeout: Duration,
    ) -> Result<Self, TransportPolicyError> {
        if interval.is_zero() {
            return Err(TransportPolicyError::ZeroDuration(
                "http2_keep_alive_interval",
            ));
        }
        if timeout.is_zero() {
            return Err(TransportPolicyError::ZeroDuration(
                "http2_keep_alive_timeout",
            ));
        }
        self.http2_keep_alive = Some((interval, timeout));
        Ok(self)
    }

    /// Builds an independent Hyper connection builder.
    #[must_use]
    pub fn builder(&self) -> Builder<TokioExecutor> {
        let mut builder = Builder::new(TokioExecutor::new());
        builder
            .http1()
            .timer(TokioTimer::new())
            .header_read_timeout(self.header_read_timeout)
            .max_buf_size(self.http1_max_buffer_bytes);
        builder
            .http2()
            .timer(TokioTimer::new())
            .max_concurrent_streams(self.http2_max_concurrent_streams.get())
            .max_header_list_size(self.http2_max_header_list_bytes.get());
        if let Some((interval, timeout)) = self.http2_keep_alive {
            builder
                .http2()
                .keep_alive_interval(interval)
                .keep_alive_timeout(timeout);
        }
        builder
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportPolicyError {
    ZeroDuration(&'static str),
    Http1BufferTooSmall { minimum: usize, actual: usize },
}

impl fmt::Display for TransportPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroDuration(field) => write!(formatter, "{field} must not be zero"),
            Self::Http1BufferTooSmall { minimum, actual } => write!(
                formatter,
                "HTTP/1 buffer must be at least {minimum} bytes, got {actual}"
            ),
        }
    }
}

impl Error for TransportPolicyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_values_that_would_disable_or_panic_hyper_limits() {
        assert!(matches!(
            HttpTransport::new(
                Duration::ZERO,
                MIN_HTTP1_BUFFER_BYTES,
                NonZeroU32::new(1).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
            Err(TransportPolicyError::ZeroDuration("header_read_timeout"))
        ));
        assert!(matches!(
            HttpTransport::new(
                Duration::from_secs(1),
                MIN_HTTP1_BUFFER_BYTES - 1,
                NonZeroU32::new(1).unwrap(),
                NonZeroU32::new(1).unwrap(),
            ),
            Err(TransportPolicyError::Http1BufferTooSmall { .. })
        ));
    }

    #[test]
    fn creates_builder_only_from_application_supplied_values() {
        let transport = HttpTransport::new(
            Duration::from_secs(1),
            MIN_HTTP1_BUFFER_BYTES,
            NonZeroU32::new(8).unwrap(),
            NonZeroU32::new(16 * 1024).unwrap(),
        )
        .unwrap()
        .with_http2_keep_alive(Duration::from_secs(30), Duration::from_secs(5))
        .unwrap();
        let _builder = transport.builder();
    }
}
