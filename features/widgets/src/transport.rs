//! A live widget's `fetch` hops, sent by this app (`opencode_widget::live::fetch` keeps the
//! rules): system TLS roots through ureq with rustls, no redirects followed here, no cookies. The
//! host is resolved once; loopback, private and link-local answers are refused outside dev mode;
//! and the connection goes to exactly the answers checked ([`Pinned`]), so a second lookup can't
//! rebind it to the local network.

use std::io::Read;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use opencode_widget::live::fetch::{FetchError, FetchErrorKind, MAX_RESPONSE, Request, Response, Transport, public};
use ureq::unversioned::resolver::{ResolvedSocketAddrs, Resolver};
use ureq::unversioned::transport::{DefaultConnector, NextTimeout};

pub struct Ureq;

fn network(message: impl Into<String>) -> FetchError {
    FetchError::new(FetchErrorKind::Network, message)
}

/// Resolves to exactly the addresses the transport checked, not a second lookup's.
#[derive(Debug)]
struct Pinned(Vec<SocketAddr>);

impl Resolver for Pinned {
    fn resolve(&self, _uri: &ureq::http::Uri, _config: &ureq::config::Config, _timeout: NextTimeout) -> Result<ResolvedSocketAddrs, ureq::Error> {
        if self.0.is_empty() {
            return Err(ureq::Error::HostNotFound);
        }
        let mut out = self.empty();
        for a in self.0.iter().take(16) {
            out.push(*a);
        }
        Ok(out)
    }
}

impl Transport for Ureq {
    fn send(&self, request: &Request, timeout: Duration) -> Result<Response, FetchError> {
        let url = url::Url::parse(&request.url).map_err(|e| network(e.to_string()))?;
        let host = url.host_str().ok_or_else(|| network("no host"))?;
        let port = url.port_or_known_default().unwrap_or(443);
        let name = host.trim_start_matches('[').trim_end_matches(']');
        let addrs: Vec<SocketAddr> = (name, port).to_socket_addrs().map_err(|e| network(format!("couldn't resolve {host}: {e}")))?.collect();
        if addrs.is_empty() {
            return Err(network(format!("{host} has no address")));
        }
        if !request.local
            && let Some(a) = addrs.iter().find(|a| !public(&a.ip()))
        {
            return Err(FetchError::new(FetchErrorKind::CapabilityDenied, format!("{host} resolves to a private address ({})", a.ip())));
        }
        let config = ureq::Agent::config_builder().timeout_global(Some(timeout)).http_status_as_error(false).max_redirects(0).save_redirect_history(false).build();
        let agent = ureq::Agent::with_parts(config, DefaultConnector::default(), Pinned(addrs));
        let mut builder = ureq::http::Request::builder().method(request.method.as_str()).uri(request.url.as_str());
        for (k, v) in &request.headers {
            builder = builder.header(k.as_str(), v.as_str());
        }
        let sent = match &request.body {
            Some(body) => agent.run(builder.body(body.clone()).map_err(|e| network(e.to_string()))?),
            None => agent.run(builder.body(()).map_err(|e| network(e.to_string()))?),
        };
        let response = sent.map_err(|e| match e {
            ureq::Error::Timeout(_) => FetchError::new(FetchErrorKind::Timeout, "the request took longer than 15 s"),
            e => network(e.to_string()),
        })?;
        let status = response.status().as_u16();
        let headers = response.headers().iter().map(|(k, v)| (k.as_str().to_ascii_lowercase(), v.to_str().unwrap_or("").to_owned())).collect();
        let mut body = Vec::new();
        let mut reader = response.into_body().into_reader().take(MAX_RESPONSE as u64 + 1);
        reader.read_to_end(&mut body).map_err(|e| network(e.to_string()))?;
        if body.len() > MAX_RESPONSE {
            return Err(FetchError::new(FetchErrorKind::TooLarge, format!("a response may be at most {} MB", MAX_RESPONSE / 1024 / 1024)));
        }
        Ok(Response { status, headers, body })
    }
}
