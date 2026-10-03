use super::*;
use pretty_assertions::assert_eq;

#[test]
fn blocking_forward_client_uses_socks_remote_dns() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let proxy = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut greeting = [0; 3];
        stream.read_exact(&mut greeting).unwrap();
        assert_eq!(greeting, [5, 1, 0]);
        stream.write_all(&[5, 0]).unwrap();
        let mut connect = [0; 5];
        stream.read_exact(&mut connect).unwrap();
        assert_eq!(&connect[..4], &[5, 1, 0, 3]);
        let mut hostname = vec![0; usize::from(connect[4])];
        stream.read_exact(&mut hostname).unwrap();
        let mut port = [0; 2];
        stream.read_exact(&mut port).unwrap();
        stream.write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0; 1];
            stream.read_exact(&mut byte).unwrap();
            request.extend_from_slice(&byte);
            assert!(request.len() <= 16_384);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nproxied")
            .unwrap();
        (
            hostname,
            u16::from_be_bytes(port),
            String::from_utf8(request)
                .unwrap()
                .lines()
                .next()
                .unwrap()
                .to_owned(),
        )
    });
    let client = build_http_client(OutboundProxyRoute::Proxy {
        url: format!("socks5h://{address}"),
        no_proxy: None,
    })
    .unwrap();
    let response = client
        .get("http://upstream.invalid:8080/v1/responses")
        .timeout(Duration::from_secs(5))
        .send()
        .unwrap()
        .text()
        .unwrap();
    assert_eq!(
        (proxy.join().unwrap(), response),
        (
            (
                b"upstream.invalid".to_vec(),
                8080,
                "GET /v1/responses HTTP/1.1".to_owned()
            ),
            "proxied".to_owned()
        )
    );
}
