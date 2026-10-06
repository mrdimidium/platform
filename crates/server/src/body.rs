// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    convert::Infallible,
    pin::Pin,
    task::{Context, Poll},
};

use axum::body::{Body, Bytes};
use http_body::{Body as HttpBody, Frame};

/// Returns an empty body with an intentionally unknown size hint.
///
/// Axum otherwise synthesizes `Content-Length: 0` after inner middleware has correctly removed
/// the field from a 304 response.
pub(crate) fn empty_unknown_size() -> Body {
    Body::new(UnknownSizeEmpty)
}

struct UnknownSizeEmpty;

impl HttpBody for UnknownSizeEmpty {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(None)
    }
}
