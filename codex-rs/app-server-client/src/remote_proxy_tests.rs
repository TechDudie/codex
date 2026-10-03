use codex_app_server_protocol::ClientRequest;
use codex_app_server_protocol::GetAccountParams;
use codex_app_server_protocol::GetAccountResponse;
use codex_app_server_protocol::JSONRPCMessage;
use codex_app_server_protocol::JSONRPCResponse;
use codex_app_server_protocol::RequestId;
use codex_http_client::HttpClientFactory;
use codex_http_client::OutboundProxyPolicy;
use pretty_assertions::assert_eq;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;
use tokio::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::accept_async;

use crate::AppServerClient;
use crate::RemoteAppServerClient;
use crate::tests::expect_remote_initialize;
use crate::tests::read_websocket_message;
use crate::tests::test_remote_connect_args;
use crate::tests::write_websocket_message;

#[tokio::test]
async fn configured_socks_proxy_carries_remote_app_server_initialization_and_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy = listener.local_addr().unwrap();
    let expected = GetAccountResponse {
        workspace_routing: None,
        account: None,
        requires_openai_auth: false,
    };
    let response = serde_json::to_value(&expected).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut greeting = [0; 3];
        stream.read_exact(&mut greeting).await.unwrap();
        assert_eq!(greeting, [5, 1, 0]);
        stream.write_all(&[5, 0]).await.unwrap();
        let mut header = [0; 5];
        stream.read_exact(&mut header).await.unwrap();
        assert_eq!(&header[..4], &[5, 1, 0, 3]);
        let mut host = vec![0; usize::from(header[4])];
        stream.read_exact(&mut host).await.unwrap();
        let port = stream.read_u16().await.unwrap();
        assert_eq!(
            (host.as_slice(), port),
            (&b"remote-app-server.invalid"[..], 18080)
        );
        stream
            .write_all(&[5, 0, 0, 1, 0, 0, 0, 0, 0, 0])
            .await
            .unwrap();
        let mut websocket = accept_async(stream).await.unwrap();
        expect_remote_initialize(&mut websocket).await;
        let JSONRPCMessage::Request(request) = read_websocket_message(&mut websocket).await else {
            panic!("expected account/read request through SOCKS tunnel");
        };
        assert_eq!(request.method, "account/read");
        write_websocket_message(
            &mut websocket,
            JSONRPCMessage::Response(JSONRPCResponse {
                id: request.id,
                result: response,
            }),
        )
        .await;
        websocket.close(/*msg*/ None).await.unwrap();
    });
    let factory = HttpClientFactory::new(OutboundProxyPolicy::RespectSystemProxy)
        .with_proxy_url(&format!("socks5h://{proxy}"))
        .unwrap();
    let remote = timeout(
        Duration::from_secs(3),
        RemoteAppServerClient::connect_with_http_client_factory(
            test_remote_connect_args("ws://remote-app-server.invalid:18080".to_string()),
            &factory,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let client = AppServerClient::Remote(remote);
    let actual: GetAccountResponse = client
        .request_typed(ClientRequest::GetAccount {
            request_id: RequestId::Integer(1),
            params: GetAccountParams {
                refresh_token: false,
            },
        })
        .await
        .unwrap();
    assert_eq!(actual, expected);
    client.shutdown().await.unwrap();
    server.await.unwrap();
}
