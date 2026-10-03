use super::*;
use codex_http_client::OutboundProxyPolicy;
use pretty_assertions::assert_eq;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tower::ServiceExt;

#[tokio::test]
async fn grpc_channel_tunnels_remote_dns_through_configured_socks_proxy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    let proxy = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut greeting = [0; 3];
        stream.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 1, 0]);
        stream.write_all(&[5, 0]).await.unwrap();
        let mut connect = [0; 5];
        stream.read_exact(&mut connect).await.unwrap();
        assert_eq!(&connect[..4], &[5, 1, 0, 3]);
        let mut hostname = vec![0; usize::from(connect[4])];
        stream.read_exact(&mut hostname).await.unwrap();
        let port = stream.read_u16().await.unwrap();
        stream
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        let mut preface = [0; 24];
        stream.read_exact(&mut preface).await.unwrap();
        (hostname, port, preface)
    });
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
        .with_proxy_url(&format!("socks5h://{proxy_address}"))
        .unwrap();
    let channel = build_channel(
        &factory,
        "http://collector.invalid:4317",
        ClientTlsConfig::new(),
        opentelemetry_otlp::OTEL_EXPORTER_OTLP_LOGS_ENDPOINT,
        opentelemetry_otlp::OTEL_EXPORTER_OTLP_TIMEOUT,
    )
    .unwrap()
    .unwrap();
    let request = http::Request::builder()
        .uri("http://collector.invalid:4317/test")
        .body(tonic::body::Body::empty())
        .unwrap();
    // The proxy closes after the HTTP/2 preface; observing it proves the channel uses the tunnel.
    let _ = tokio::time::timeout(Duration::from_secs(5), channel.oneshot(request)).await;
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .unwrap()
            .unwrap(),
        (
            b"collector.invalid".to_vec(),
            4317,
            *b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n"
        )
    );
}
