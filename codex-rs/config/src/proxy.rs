use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;
use std::fmt;

/// Explicit proxy routing for Codex-owned outbound HTTP and WebSocket requests.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ProxyConfigToml {
    /// Proxy URL. Supports `http`, `https`, `socks5` (local DNS), and `socks5h`
    /// (proxy DNS), with optional username/password authentication. SOCKS5 uses
    /// port 1080 when omitted. Takes precedence over environment and system
    /// proxies, without direct fallback.
    pub url: String,
}

impl fmt::Debug for ProxyConfigToml {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProxyConfigToml")
            .field("url", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
