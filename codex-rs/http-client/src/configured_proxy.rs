use crate::BuildRouteAwareHttpClientError;
use crate::ClientRouteClass;

/// Validates configuration without resolving the proxy host or exposing credentials in errors.
pub(crate) fn validate_proxy_url(url: &str) -> Result<String, BuildRouteAwareHttpClientError> {
    let invalid = || BuildRouteAwareHttpClientError::InvalidProxyConfig {
        route_class: ClientRouteClass::Other,
    };
    let mut parsed = reqwest::Url::parse(url).map_err(|_| invalid())?;
    if !matches!(parsed.scheme(), "http" | "https" | "socks5" | "socks5h")
        || parsed.host_str().is_none_or(str::is_empty)
        || parsed.port() == Some(0)
        || !matches!(parsed.path(), "" | "/")
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(invalid());
    }
    for credential in [parsed.username(), parsed.password().unwrap_or_default()] {
        let mut bytes = credential.bytes();
        while let Some(byte) = bytes.next() {
            if byte == b'%'
                && !(bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit())
                    && bytes.next().is_some_and(|byte| byte.is_ascii_hexdigit()))
            {
                return Err(invalid());
            }
        }
        let decoded = urlencoding::decode(credential).map_err(|_| invalid())?;
        if matches!(parsed.scheme(), "socks5" | "socks5h") && decoded.len() > 255 {
            return Err(invalid());
        }
    }
    if matches!(parsed.scheme(), "socks5" | "socks5h") && parsed.port().is_none() {
        parsed.set_port(Some(1080)).map_err(|_| invalid())?;
    }
    reqwest::Proxy::all(parsed.clone()).map_err(|_| invalid())?;
    Ok(parsed.to_string())
}

#[cfg(test)]
#[path = "configured_proxy_tests.rs"]
mod tests;
