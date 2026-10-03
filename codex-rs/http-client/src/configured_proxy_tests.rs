use std::io::Read;
use std::io::Write;
use std::net::IpAddr;
use std::net::SocketAddr;
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;
use std::time::Instant;

use pretty_assertions::assert_eq;

use crate::ClientRouteClass;
use crate::HttpClientBuilder;
use crate::HttpClientFactory;
use crate::HttpClientTlsConfig;
use crate::OutboundProxyPolicy;
use crate::OutboundProxyRoute;
use crate::RouteAwareClientPool;

const OK_RESPONSE: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

#[derive(Clone, Copy)]
enum ClientKind {
    Fixed,
    Pooled,
}

#[tokio::test]
async fn fixed_and_pooled_socks_clients_preserve_dns_addresses_and_credentials() {
    for kind in [ClientKind::Fixed, ClientKind::Pooled] {
        for (scheme, host) in [
            ("socks5h", "unresolvable.invalid"),
            ("socks5", "localhost"),
            ("socks5h", "192.0.2.1"),
            ("socks5h", "[2001:db8::7]"),
        ] {
            let (proxy, server) = spawn_proxy(
                vec![ProxyReply::Http(OK_RESPONSE.to_string())],
                /*tls*/ None,
            );
            let factory = HttpClientFactory::new(OutboundProxyPolicy::RespectSystemProxy)
                .with_proxy_url(&format!("{scheme}://user%40name:pass%3Aword@{proxy}"))
                .unwrap();
            let url = format!("http://{host}:18080/probe");
            let client = match kind {
                ClientKind::Fixed => factory.build_client(&url, ClientRouteClass::Api).unwrap(),
                ClientKind::Pooled => {
                    RouteAwareClientPool::new(factory, ClientRouteClass::Api).into_client()
                }
            };
            assert_eq!(
                client
                    .get(url)
                    .timeout(Duration::from_secs(3))
                    .send()
                    .await
                    .unwrap()
                    .text()
                    .await
                    .unwrap(),
                "ok"
            );
            let [captured]: [SocksRequest; 1] = server.join().unwrap().try_into().unwrap();
            let address = match host {
                "unresolvable.invalid" => SocksAddress::Domain(host.to_string()),
                "localhost" => {
                    let SocksAddress::Ip(address) = captured.address else {
                        panic!("socks5 must send a locally resolved address");
                    };
                    assert!(address.is_loopback());
                    SocksAddress::Ip(address)
                }
                literal => SocksAddress::Ip(literal.trim_matches(['[', ']']).parse().unwrap()),
            };
            assert_eq!(
                captured,
                SocksRequest {
                    auth: Some(("user@name".to_string(), "pass:word".to_string())),
                    address,
                    port: 18080,
                }
            );
        }
    }
}

#[tokio::test]
async fn https_socks_tunnel_uses_configured_root_and_original_hostname() {
    codex_utils_rustls_provider::ensure_rustls_crypto_provider();
    let certified =
        rcgen::generate_simple_self_signed(vec!["unresolvable.invalid".to_string()]).unwrap();
    let tls = HttpClientTlsConfig::default()
        .with_root_certificate_pem(certified.cert.pem().as_bytes())
        .unwrap();
    let server_tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![certified.cert.der().clone()],
            certified.signing_key.into(),
        )
        .unwrap();
    let (proxy, server) = spawn_proxy(
        vec![ProxyReply::Http(OK_RESPONSE.to_string())],
        Some(Arc::new(server_tls)),
    );
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
        .with_proxy_url(&format!("socks5h://{proxy}"))
        .unwrap();
    let client = HttpClientBuilder::new().build_with_tls(&factory, ClientRouteClass::Api, tls);
    assert_eq!(
        client
            .get("https://unresolvable.invalid:18443/probe")
            .timeout(Duration::from_secs(3))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "ok"
    );
    assert_eq!(
        server.join().unwrap(),
        vec![SocksRequest {
            auth: None,
            address: SocksAddress::Domain("unresolvable.invalid".to_string()),
            port: 18443,
        }]
    );
}

#[tokio::test]
async fn redirects_keep_explicit_proxy_with_legacy_direct_fallback() {
    let destination = "http://redirect-target.invalid:18081/probe";
    let (proxy, server) = spawn_proxy(
        vec![
            ProxyReply::Http(format!(
                "HTTP/1.1 302 Found\r\nLocation: {destination}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )),
            ProxyReply::Http(OK_RESPONSE.to_string()),
        ],
        /*tls*/ None,
    );
    let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
        .with_proxy_url(&format!("socks5h://{proxy}"))
        .unwrap()
        .with_system_proxy_fallback();
    assert!(!factory.allows_system_proxy_fallback());
    let pool = RouteAwareClientPool::new(factory, ClientRouteClass::Api)
        .with_legacy_direct_proxy_and_custom_ca_fallback();
    let response = pool
        .get("http://redirect-start.invalid:18080/probe")
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .unwrap();
    assert_eq!(response.url().as_str(), destination);
    assert_eq!(response.text().await.unwrap(), "ok");
    assert_eq!(
        server.join().unwrap(),
        vec![
            SocksRequest {
                auth: None,
                address: SocksAddress::Domain("redirect-start.invalid".to_string()),
                port: 18080
            },
            SocksRequest {
                auth: None,
                address: SocksAddress::Domain("redirect-target.invalid".to_string()),
                port: 18081
            },
        ]
    );
}

#[tokio::test]
async fn rejected_and_unavailable_socks_proxies_never_connect_directly() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let url = format!("http://{}/probe", target.local_addr().unwrap());
    let (rejected, server) = spawn_proxy(vec![ProxyReply::Reject], /*tls*/ None);
    let unavailable = TcpListener::bind("127.0.0.1:0").unwrap();
    let unavailable_address = unavailable.local_addr().unwrap();
    drop(unavailable);
    for proxy in [rejected, unavailable_address] {
        let factory = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
            .with_proxy_url(&format!("socks5h://{proxy}"))
            .unwrap()
            .with_system_proxy_fallback();
        let client = RouteAwareClientPool::new(factory, ClientRouteClass::Api)
            .with_legacy_direct_proxy_and_custom_ca_fallback()
            .into_client();
        assert!(
            client
                .get(&url)
                .timeout(Duration::from_secs(3))
                .send()
                .await
                .is_err()
        );
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
    server.join().unwrap();
}

#[test]
fn invalid_proxy_urls_and_debug_output_protect_credentials() {
    for url in [
        "socks4://user:secret@proxy.example",
        "socks5h://user:secret@proxy.example:0",
        "socks5h://user:secret@proxy.example/path",
        "socks5h://user:secret@proxy.example?token=secret",
        "socks5h://user:secret@proxy.example#secret",
        "socks5h://user:secret@",
        "socks5h://user:secret%FF@proxy.example",
        "socks5h://user:secret%ZZ@proxy.example",
    ] {
        let error = HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
            .with_proxy_url(url)
            .unwrap_err();
        assert!(!format!("{error:?} {error}").contains("secret"));
    }
    for credentials in [
        format!("{}:secret", "u".repeat(256)),
        format!("user:{}", "p".repeat(256)),
    ] {
        assert!(
            HttpClientFactory::new(OutboundProxyPolicy::ReqwestDefault)
                .with_proxy_url(&format!("socks5h://{credentials}@proxy.example"))
                .is_err()
        );
    }
    let factory = HttpClientFactory::new(OutboundProxyPolicy::RespectSystemProxy)
        .with_proxy_url("socks5h://user:secret@private.proxy.example")
        .unwrap();
    for destination in ["http://target.invalid/", "wss://target.invalid/"] {
        let route = factory.resolve_proxy_route(destination);
        assert_eq!(
            route,
            OutboundProxyRoute::Proxy {
                url: "socks5h://user:secret@private.proxy.example:1080".to_string(),
                no_proxy: None,
            }
        );
        let debug = format!("{factory:?} {route:?}");
        assert!(!debug.contains("secret") && !debug.contains("private.proxy.example"));
    }
}

#[derive(Debug, PartialEq, Eq)]
enum SocksAddress {
    Ip(IpAddr),
    Domain(String),
}

#[derive(Debug, PartialEq, Eq)]
struct SocksRequest {
    auth: Option<(String, String)>,
    address: SocksAddress,
    port: u16,
}

enum ProxyReply {
    Http(String),
    Reject,
}

fn spawn_proxy(
    responses: Vec<ProxyReply>,
    tls: Option<Arc<rustls::ServerConfig>>,
) -> (SocketAddr, JoinHandle<Vec<SocksRequest>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        responses
            .into_iter()
            .map(|response| {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                Instant::now() < deadline,
                                "SOCKS proxy did not receive a connection"
                            );
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) => panic!("SOCKS proxy failed to accept: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut greeting = [0; 2];
                stream.read_exact(&mut greeting).unwrap();
                assert_eq!(greeting[0], 5);
                let mut methods = vec![0; usize::from(greeting[1])];
                stream.read_exact(&mut methods).unwrap();
                let auth = if methods.contains(&2) {
                    stream.write_all(&[5, 2]).unwrap();
                    let mut version = [0];
                    stream.read_exact(&mut version).unwrap();
                    assert_eq!(version, [1]);
                    let credentials = (
                        read_socks_string(&mut stream),
                        read_socks_string(&mut stream),
                    );
                    stream.write_all(&[1, 0]).unwrap();
                    Some(credentials)
                } else {
                    assert!(methods.contains(&0));
                    stream.write_all(&[5, 0]).unwrap();
                    None
                };
                let mut header = [0; 4];
                stream.read_exact(&mut header).unwrap();
                assert_eq!(&header[..3], &[5, 1, 0]);
                let address = match header[3] {
                    1 => {
                        let mut bytes = [0; 4];
                        stream.read_exact(&mut bytes).unwrap();
                        SocksAddress::Ip(bytes.into())
                    }
                    4 => {
                        let mut bytes = [0; 16];
                        stream.read_exact(&mut bytes).unwrap();
                        SocksAddress::Ip(bytes.into())
                    }
                    3 => SocksAddress::Domain(read_socks_string(&mut stream)),
                    kind => panic!("unexpected SOCKS address type {kind}"),
                };
                let mut port = [0; 2];
                stream.read_exact(&mut port).unwrap();
                match response {
                    ProxyReply::Reject => {
                        stream.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap()
                    }
                    ProxyReply::Http(response) => {
                        stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
                        match &tls {
                            Some(config) => {
                                let connection =
                                    rustls::ServerConnection::new(Arc::clone(config)).unwrap();
                                let mut stream = rustls::StreamOwned::new(connection, stream);
                                respond_http(&mut stream, &response);
                            }
                            None => respond_http(&mut stream, &response),
                        }
                    }
                }
                SocksRequest {
                    auth,
                    address,
                    port: u16::from_be_bytes(port),
                }
            })
            .collect()
    });
    (address, server)
}

fn read_socks_string(stream: &mut impl Read) -> String {
    let mut length = [0];
    stream.read_exact(&mut length).unwrap();
    let mut bytes = vec![0; usize::from(length[0])];
    stream.read_exact(&mut bytes).unwrap();
    String::from_utf8(bytes).unwrap()
}

fn respond_http(stream: &mut (impl Read + Write), response: &str) {
    let mut header = Vec::new();
    while !header.ends_with(b"\r\n\r\n") {
        assert!(
            header.len() < 8192,
            "HTTP request headers exceed the test limit"
        );
        let mut byte = [0];
        stream.read_exact(&mut byte).unwrap();
        header.push(byte[0]);
    }
    assert!(header.starts_with(b"GET /probe HTTP/1.1\r\n"));
    stream.write_all(response.as_bytes()).unwrap();
    stream.flush().unwrap();
}
