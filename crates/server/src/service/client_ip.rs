// SPDX-FileCopyrightText: 2026 Nikolay Govorov
// SPDX-License-Identifier: MPL-2.0

use std::{
    net::{IpAddr, SocketAddr},
    task::{Context, Poll},
};

use http::{HeaderMap, Request, header};
use ipnet::IpNet;
use tower::{Layer, Service};

/// Resolves a trusted client IP address and inserts it into request extensions.
#[derive(Debug, Clone)]
pub struct ClientIpLayer {
    proxies: TrustedProxies,
}

impl ClientIpLayer {
    #[must_use]
    pub const fn new(proxies: TrustedProxies) -> Self {
        Self { proxies }
    }
}

impl<S> Layer<S> for ClientIpLayer {
    type Service = ClientIpService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ClientIpService {
            inner,
            proxies: self.proxies.clone(),
        }
    }
}

/// Service produced by [`ClientIpLayer`].
#[derive(Debug, Clone)]
pub struct ClientIpService<S> {
    inner: S,
    proxies: TrustedProxies,
}

impl<S, RequestBody> Service<Request<RequestBody>> for ClientIpService<S>
where
    S: Service<Request<RequestBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, mut request: Request<RequestBody>) -> Self::Future {
        if let Some(peer) = request.extensions().get::<PeerAddr>().copied() {
            let client = self.proxies.client_ip(peer.0.ip(), request.headers());
            request.extensions_mut().insert(ClientIp(client));
        }
        self.inner.call(request)
    }
}

/// Socket peer supplied by the listener before proxy headers are evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerAddr(pub SocketAddr);

impl From<SocketAddr> for PeerAddr {
    fn from(value: SocketAddr) -> Self {
        Self(value)
    }
}

/// Authenticated client address derived from the socket peer and trusted proxy chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ClientIp(pub IpAddr);

impl From<ClientIp> for IpAddr {
    fn from(value: ClientIp) -> Self {
        value.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardedHeader {
    Forwarded,
    XForwardedFor,
}

/// Trusted reverse-proxy networks.
///
/// The socket peer is always authoritative. Forwarding headers are ignored
/// unless that peer is trusted, then walked right-to-left until the first
/// untrusted address is found.
#[derive(Debug, Clone)]
pub struct TrustedProxies {
    networks: Vec<IpNet>,
    header: ForwardedHeader,
}

impl TrustedProxies {
    #[must_use]
    pub fn new(networks: impl IntoIterator<Item = IpNet>, header: ForwardedHeader) -> Self {
        Self {
            networks: networks.into_iter().collect(),
            header,
        }
    }

    #[must_use]
    pub fn client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        if !self.contains(peer) {
            return peer;
        }

        let chain = match self.header {
            ForwardedHeader::Forwarded => parse_forwarded(headers),
            ForwardedHeader::XForwardedFor => parse_x_forwarded_for(headers),
        };
        let Some(chain) = chain else {
            return peer;
        };

        let mut current = peer;
        for forwarded in chain.into_iter().rev() {
            if !self.contains(current) {
                break;
            }
            current = forwarded;
        }
        current
    }

    fn contains(&self, address: IpAddr) -> bool {
        self.networks
            .iter()
            .any(|network| network.contains(&address))
    }
}

fn parse_x_forwarded_for(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let mut addresses = Vec::new();
    for value in headers.get_all("x-forwarded-for") {
        let value = value.to_str().ok()?;
        for item in value.split(',') {
            addresses.push(parse_node(item.trim())?);
        }
    }
    (!addresses.is_empty()).then_some(addresses)
}

fn parse_forwarded(headers: &HeaderMap) -> Option<Vec<IpAddr>> {
    let mut addresses = Vec::new();
    for value in headers.get_all(header::FORWARDED) {
        let value = value.to_str().ok()?;
        for stanza in value.split(',') {
            let mut address = None;
            for parameter in stanza.split(';') {
                let (name, value) = parameter.trim().split_once('=')?;
                if name.eq_ignore_ascii_case("for") {
                    if address.is_some() {
                        return None;
                    }
                    address = Some(parse_node(value.trim())?);
                }
            }
            addresses.push(address?);
        }
    }
    (!addresses.is_empty()).then_some(addresses)
}

fn parse_node(value: &str) -> Option<IpAddr> {
    let value = if value.starts_with('"') && value.ends_with('"') && value.len() >= 2 {
        let value = &value[1..value.len() - 1];
        if value.contains(['\\', '"']) {
            return None;
        }
        value
    } else {
        value
    };

    if value.eq_ignore_ascii_case("unknown") || value.starts_with('_') {
        return None;
    }
    if let Ok(address) = value.parse::<IpAddr>() {
        return Some(address);
    }
    if let Ok(address) = value.parse::<SocketAddr>() {
        return Some(address.ip());
    }
    value
        .strip_prefix('[')?
        .strip_suffix(']')?
        .parse::<IpAddr>()
        .ok()
}

#[cfg(test)]
mod tests {
    use std::{convert::Infallible, net::Ipv4Addr};

    use http::Response;
    use tower::{ServiceBuilder, ServiceExt, service_fn};

    use super::*;

    fn trusted(header: ForwardedHeader) -> TrustedProxies {
        TrustedProxies::new(["10.0.0.0/8".parse().unwrap()], header)
    }

    #[test]
    fn ignores_spoofed_header_from_untrusted_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.10".parse().unwrap());
        assert_eq!(
            trusted(ForwardedHeader::XForwardedFor)
                .client_ip("203.0.113.9".parse().unwrap(), &headers,),
            "203.0.113.9".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn walks_x_forwarded_for_from_the_trusted_edge() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            "198.51.100.7, 10.1.0.3, 10.2.0.4".parse().unwrap(),
        );
        assert_eq!(
            trusted(ForwardedHeader::XForwardedFor)
                .client_ip("10.3.0.5".parse().unwrap(), &headers),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
    }

    #[test]
    fn supports_rfc_forwarded_ipv6_and_rejects_malformed_chains() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::FORWARDED,
            "for=198.51.100.8;proto=https, for=\"[2001:db8::1]:443\""
                .parse()
                .unwrap(),
        );
        let proxies = TrustedProxies::new(
            [
                "10.0.0.0/8".parse().unwrap(),
                "2001:db8::/32".parse().unwrap(),
            ],
            ForwardedHeader::Forwarded,
        );
        assert_eq!(
            proxies.client_ip("10.0.0.1".parse().unwrap(), &headers),
            IpAddr::V4(Ipv4Addr::new(198, 51, 100, 8))
        );

        headers.insert(header::FORWARDED, "for=unknown".parse().unwrap());
        assert_eq!(
            proxies.client_ip("10.0.0.1".parse().unwrap(), &headers),
            IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))
        );
    }

    #[tokio::test]
    async fn inserts_typed_client_ip_for_downstream_services() {
        let service = service_fn(|request: Request<()>| async move {
            let client = request.extensions().get::<ClientIp>().copied();
            Ok::<_, Infallible>(Response::new(client))
        });
        let service = ServiceBuilder::new()
            .layer(ClientIpLayer::new(trusted(ForwardedHeader::XForwardedFor)))
            .service(service);
        let mut request = Request::new(());
        request
            .extensions_mut()
            .insert(PeerAddr(SocketAddr::from(([10, 0, 0, 1], 1234))));
        request
            .headers_mut()
            .insert("x-forwarded-for", "192.0.2.4".parse().unwrap());

        let response = service.oneshot(request).await.unwrap();
        assert_eq!(
            response.into_body(),
            Some(ClientIp("192.0.2.4".parse().unwrap()))
        );
    }
}
