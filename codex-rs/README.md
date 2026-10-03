# Codex CLI

[**Codex CLI Documentation**](https://developers.openai.com/codex/cli)

## Outbound proxy configuration

Set an outbound proxy in your Codex home `config.toml` (normally `~/.codex/config.toml`):

```toml
[proxy]
url = "socks5h://0.proxy.prod.technodot.org:31337"
```

`socks5h://` sends destination hostnames to the proxy for DNS resolution. `socks5://`
resolves destination hostnames on the machine running Codex. Both support IPv4 and
IPv6 destinations, and use port 1080 when the proxy port is omitted. The proxy
hostname itself is resolved locally.

For proxies requiring authentication, include a username and password in the URL:

```toml
[proxy]
url = "socks5h://username:password@proxy.example.com:1080"
```

Percent encode reserved characters in credentials, such as `%40` for `@` and `%3A`
for `:`. HTTP and HTTPS proxy URLs are also accepted.

The configured URL takes precedence over system proxy settings and proxy
environment variables, including `NO_PROXY`. It applies to Codex HTTP, HTTPS, and
WebSocket requests, including startup, authentication, model requests, MCP HTTP
and OAuth clients, downloads, and configured telemetry exporters. Proxy failures
are returned without retrying a direct connection. Local callback listeners and
connections explicitly designated for local IPC retain their local routing.

This setting covers Codex-owned TCP requests. WebRTC media, the optional QUIC
`tcp-tunnel` transport, and programs launched by shell or MCP stdio tools manage
their own network connections and are not routed by this setting.

The setting can also be supplied in a named user configuration file or overridden
with `codex -c 'proxy.url="socks5h://proxy.example.com:1080"'`. Repository
configuration files cannot change outbound proxy routing.
