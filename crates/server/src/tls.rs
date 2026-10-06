// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::sync::Arc;

use arc_swap::ArcSwap;
use rustls::{
    ServerConfig,
    server::{ClientHello, NoServerSessionStorage, ProducesTickets, ResolvesServerCert},
    sign::CertifiedKey,
};

/// A rustls certificate resolver that supports atomic certificate rotation.
///
/// Parsing and validating certificate/key files belongs to the calling service;
/// only a complete [`CertifiedKey`] can be installed here.
#[derive(Debug)]
pub struct ReloadingServerCert {
    current: ArcSwap<CertifiedKey>,
}

impl ReloadingServerCert {
    #[must_use]
    pub fn new(initial: Arc<CertifiedKey>) -> Self {
        Self {
            current: ArcSwap::new(initial),
        }
    }

    pub fn replace(&self, certificate: Arc<CertifiedKey>) {
        self.current.store(certificate);
    }

    #[must_use]
    pub fn current(&self) -> Arc<CertifiedKey> {
        self.current.load_full()
    }
}

impl ResolvesServerCert for ReloadingServerCert {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}

/// Disables TLS session resumption for services whose authorization must run on
/// every new connection.
///
/// This is a safe fallback for stateful mTLS authorization. A service may keep
/// resumption enabled only when it performs revocation-aware authorization
/// independently of the original handshake.
pub fn disable_session_resumption(config: &mut ServerConfig) {
    config.session_storage = Arc::new(NoServerSessionStorage {});
    config.ticketer = Arc::new(DisabledTickets);
    config.send_tls13_tickets = 0;
}

#[derive(Debug)]
struct DisabledTickets;

impl ProducesTickets for DisabledTickets {
    fn enabled(&self) -> bool {
        false
    }

    fn lifetime(&self) -> u32 {
        0
    }

    fn encrypt(&self, _plain: &[u8]) -> Option<Vec<u8>> {
        None
    }

    fn decrypt(&self, _cipher: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_ticket_source_cannot_resume_sessions() {
        let tickets = DisabledTickets;
        assert!(!tickets.enabled());
        assert_eq!(tickets.lifetime(), 0);
        assert!(tickets.encrypt(b"session").is_none());
        assert!(tickets.decrypt(b"ticket").is_none());
    }
}
