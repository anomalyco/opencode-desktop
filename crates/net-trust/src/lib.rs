//! The certificate check of every HTTPS request and `wss://` socket the app makes: the operating
//! system's (Windows' certificate store and chain engine, macOS' Security framework, the
//! distribution's CA bundle on Linux), through `rustls-platform-verifier`, as Desktop's
//! Chromium does. A network that inspects TLS with its own root installed in the system store (a
//! company's) then works, where ureq's default (the Mozilla list compiled into the binary)
//! refuses it as an unknown issuer.
//!
//! Every ureq agent starts from [`agent_config`] (`scripts/guardrails/net-trust.mjs` fails a
//! `ureq::Agent::config_builder()` or a default agent anywhere else), and a WebSocket's TLS takes
//! [`rustls_client_config`].

#![forbid(unsafe_code)]

#[cfg(not(target_arch = "wasm32"))]
use std::sync::{Arc, OnceLock};

/// A ureq agent's configuration, checking certificates as the operating system does; the
/// caller adds its timeouts and the rest.
#[cfg(not(target_arch = "wasm32"))]
pub fn agent_config() -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    ureq::Agent::config_builder().tls_config(tls_config())
}

/// A page's requests: the browser checks their certificates itself.
#[cfg(target_arch = "wasm32")]
pub fn agent_config() -> ureq_web::Config {
    ureq_web::Agent::config_builder()
}

/// ureq's TLS settings with the operating system's certificate check.
#[cfg(not(target_arch = "wasm32"))]
pub fn tls_config() -> ureq::tls::TlsConfig {
    ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build()
}

/// A rustls client configuration with the operating system's certificate check, for a
/// WebSocket's own TLS (`tungstenite::Connector::Rustls`). Made once. `None` when the platform's
/// verifier can't be set up, and the socket then keeps its library's own roots.
#[cfg(not(target_arch = "wasm32"))]
pub fn rustls_client_config() -> Option<Arc<rustls::ClientConfig>> {
    static CONFIG: OnceLock<Option<Arc<rustls::ClientConfig>>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            use rustls_platform_verifier::ConfigVerifierExt as _;
            rustls::ClientConfig::with_platform_verifier().ok().map(Arc::new)
        })
        .clone()
}
