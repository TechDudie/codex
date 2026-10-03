//! Explicit proxy tunnels for tonic exporters; tonic retains destination TLS and SNI.

use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyRoute;
use codex_http_client::build_rustls_client_config_with_custom_ca;
use hyper_util::rt::TokioIo;
use std::error::Error;
use std::io;
use tokio::io::AsyncRead;
use tokio::io::AsyncWrite;
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_tungstenite::proxy::connect_via_proxy;
use tokio_tungstenite::tungstenite::proxy::ProxyConfig;
use tokio_tungstenite::tungstenite::proxy::ProxyScheme;
use tonic::transport::Channel;
use tonic::transport::ClientTlsConfig;
use tonic::transport::Endpoint;
use tower::service_fn;

/// A plain or TLS proxy tunnel whose destination TLS is applied by tonic.
trait ProxyIo: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> ProxyIo for T {}

pub(crate) fn build_channel(
    factory: &HttpClientFactory,
    endpoint: &str,
    tls: ClientTlsConfig,
    endpoint_var: &str,
    timeout_var: &str,
) -> Result<Option<Channel>, Box<dyn Error>> {
    if !factory.has_explicit_proxy() {
        return Ok(None);
    }
    // Match tonic's endpoint resolution: nonempty builder configuration wins, then the signal
    // environment variable, then the general OTLP environment variable, then the gRPC default.
    let endpoint = if endpoint.is_empty() {
        std::env::var(endpoint_var)
            .or_else(|_| std::env::var(opentelemetry_otlp::OTEL_EXPORTER_OTLP_ENDPOINT))
            .unwrap_or_else(|_| "http://localhost:4317".to_owned())
    } else {
        endpoint.to_owned()
    };
    let OutboundProxyRoute::Proxy { url, .. } = factory.resolve_proxy_route(&endpoint) else {
        return Err(io::Error::other("explicit OTLP proxy route is unavailable").into());
    };
    let mut url = reqwest::Url::parse(&url)
        .map_err(|_| io::Error::other("invalid OTLP proxy configuration"))?;
    let proxy_tls = if url.scheme() == "https" {
        let port = url.port_or_known_default().unwrap_or(443);
        url.set_scheme("http")
            .map_err(|()| io::Error::other("invalid OTLP proxy scheme"))?;
        url.set_port(Some(port))
            .map_err(|()| io::Error::other("invalid OTLP proxy port"))?;
        Some(build_rustls_client_config_with_custom_ca()?)
    } else {
        None
    };
    let proxy = ProxyConfig::parse(url.as_str())
        .map_err(|_| io::Error::other("invalid OTLP proxy configuration"))?;
    let timeout = crate::otlp::resolve_otlp_timeout(timeout_var);
    let channel = Endpoint::from_shared(endpoint)?
        .tls_config(tls)?
        .timeout(timeout)
        .connect_timeout(timeout)
        .connect_with_connector_lazy(service_fn(move |target: http::Uri| {
            let proxy = proxy.clone();
            let proxy_tls = proxy_tls.clone();
            async move {
                let host = target
                    .host()
                    .ok_or_else(|| io::Error::other("OTLP endpoint has no host"))?;
                let host = host.trim_start_matches('[').trim_end_matches(']');
                let port = target
                    .port_u16()
                    .unwrap_or(if target.scheme_str() == Some("https") {
                        443
                    } else {
                        80
                    });
                let host = match proxy.scheme {
                    ProxyScheme::Http => target.host().unwrap_or(host).to_owned(),
                    ProxyScheme::Socks5h => host.to_owned(),
                    ProxyScheme::Socks5 => tokio::net::lookup_host((host, port))
                        .await?
                        .next()
                        .ok_or_else(|| io::Error::other("SOCKS5 destination did not resolve"))?
                        .ip()
                        .to_string(),
                };
                let proxy_host = proxy.host.trim_start_matches('[').trim_end_matches(']');
                let stream = TcpStream::connect((proxy_host, proxy.port)).await?;
                let stream: Box<dyn ProxyIo> = match proxy_tls {
                    Some(config) => {
                        let name = ServerName::try_from(proxy_host.to_owned())
                            .map_err(|_| io::Error::other("invalid OTLP proxy TLS name"))?;
                        Box::new(TlsConnector::from(config).connect(name, stream).await?)
                    }
                    None => Box::new(stream),
                };
                let stream = connect_via_proxy(stream, &proxy, &host, port)
                    .await
                    .map_err(|_| io::Error::other("OTLP proxy tunnel failed"))?;
                Ok::<_, io::Error>(TokioIo::new(stream))
            }
        }));
    Ok(Some(channel))
}

#[cfg(test)]
#[path = "grpc_proxy_tests.rs"]
mod tests;
