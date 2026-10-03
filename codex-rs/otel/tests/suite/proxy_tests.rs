use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use codex_otel::OtelExporter;
use codex_otel::OtelHttpProtocol;
use codex_otel::OtelProvider;
use codex_otel::OtelSettings;
use opentelemetry::trace::Tracer as _;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tracing_subscriber::layer::SubscriberExt;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn configured_proxy_carries_logs_metrics_and_traces() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_address = listener.local_addr().unwrap();
    let proxy = tokio::spawn(async move {
        let mut targets = Vec::new();
        for _ in 0..3 {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut header = Vec::new();
            while !header.ends_with(b"\r\n\r\n") {
                header.push(stream.read_u8().await.unwrap());
                assert!(header.len() <= 16_384);
            }
            let header = String::from_utf8(header).unwrap();
            targets.push(header.split_whitespace().nth(1).unwrap().to_owned());
            let length = header
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap())
                })
                .unwrap();
            let mut body = vec![0; length];
            stream.read_exact(&mut body).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
        }
        targets.sort();
        targets
    });
    let exporter = |signal| OtelExporter::OtlpHttp {
        endpoint: format!("http://collector.invalid/v1/{signal}"),
        headers: HashMap::new(),
        protocol: OtelHttpProtocol::Json,
        tls: None,
    };
    let provider = OtelProvider::try_new(&OtelSettings {
        http_client_factory: HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
            .with_proxy_url(&format!("http://{proxy_address}"))
            .unwrap(),
        environment: "test".to_string(),
        service_name: "codex-cli".to_string(),
        service_version: env!("CARGO_PKG_VERSION").to_string(),
        codex_home: PathBuf::from("."),
        exporter: exporter("logs"),
        trace_exporter: exporter("traces"),
        metrics_exporter: exporter("metrics"),
        runtime_metrics: false,
        span_attributes: BTreeMap::new(),
        tracestate: BTreeMap::new(),
    })
    .unwrap()
    .unwrap();
    let subscriber = tracing_subscriber::registry().with(provider.logger_layer().unwrap());
    tracing::subscriber::with_default(subscriber, || {
        tracing::event!(target: "codex_otel.log_only", tracing::Level::INFO, "proxied log");
    });
    drop(provider.tracer.as_ref().unwrap().start("proxied span"));
    provider
        .metrics
        .as_ref()
        .unwrap()
        .counter("codex.proxy", /*inc*/ 1, &[])
        .unwrap();
    provider
        .shutdown_with_timeout(Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), proxy)
            .await
            .unwrap()
            .unwrap(),
        vec![
            "http://collector.invalid/v1/logs",
            "http://collector.invalid/v1/metrics",
            "http://collector.invalid/v1/traces",
        ]
    );
}
