use super::Config;
use super::ConfigBuilder;
use super::ConfigOverrides;
use super::ConfigToml;
use super::LoaderOverrides;
use super::resolve_bootstrap_auth_route_config;
use super::resolve_bootstrap_http_client_factory;
use codex_config::CONFIG_TOML_FILE;
use codex_http_client::OutboundProxyRoute;
use codex_utils_absolute_path::AbsolutePathBuf;
use core_test_support::TempDirExt;
use pretty_assertions::assert_eq;
use std::io::ErrorKind;
use tempfile::TempDir;
use toml::Value as TomlValue;

#[tokio::test]
async fn configured_proxy_matches_bootstrap_and_auth_routing() -> std::io::Result<()> {
    for scheme in ["socks5", "socks5h", "http", "https"] {
        for respect_system_proxy in [false, true] {
            let codex_home = TempDir::new()?;
            let url = format!("{scheme}://user:password@proxy.example:1080");
            let input = format!(
                "[proxy]\nurl = \"{url}\"\n[features]\nrespect_system_proxy = {respect_system_proxy}\nsystem_proxy_fallback = true\n"
            );
            std::fs::write(codex_home.path().join(CONFIG_TOML_FILE), &input)?;
            let config = ConfigBuilder::without_managed_config_for_tests()
                .codex_home(codex_home.path().to_path_buf())
                .build()
                .await?;
            let bootstrap: ConfigToml = toml::from_str(&input).expect("valid proxy config");
            let factory = resolve_bootstrap_http_client_factory(
                &bootstrap, /*feature_requirements*/ None,
            )?;
            assert_eq!(config.http_client_factory(), factory);
            assert_eq!(config.auth_route_config().http_client_factory(), &factory);
            assert_eq!(
                resolve_bootstrap_auth_route_config(
                    &bootstrap, /*feature_requirements*/ None
                )?
                .http_client_factory(),
                &factory
            );
            assert!(!factory.allows_system_proxy_fallback());
            for destination in [
                "http://localhost/",
                "https://api.openai.com/v1/responses",
                "wss://api.openai.com/v1/responses",
            ] {
                assert_eq!(
                    factory.resolve_proxy_route(destination),
                    OutboundProxyRoute::Proxy {
                        url: url::Url::parse(&url)
                            .expect("valid configured proxy URL")
                            .to_string(),
                        no_proxy: None,
                    }
                );
            }
        }
    }
    Ok(())
}

#[tokio::test]
async fn invalid_proxy_fails_config_and_bootstrap_without_exposing_credentials()
-> std::io::Result<()> {
    for url in [
        "ftp://private-user:private-password@private-proxy.example:1080",
        "socks5h://private-user:private-password@private-proxy.example:invalid",
    ] {
        let codex_home = TempDir::new()?;
        let cfg: ConfigToml = toml::from_str(&format!("[proxy]\nurl = \"{url}\"\n"))
            .expect("valid TOML with invalid proxy URL");
        let bootstrap_error =
            resolve_bootstrap_http_client_factory(&cfg, /*feature_requirements*/ None)
                .expect_err("invalid proxy must fail bootstrap");
        let config_error = Config::load_from_base_config_with_overrides(
            cfg,
            ConfigOverrides::default(),
            codex_home.abs(),
        )
        .await
        .expect_err("invalid proxy must fail config loading");
        assert_eq!(config_error.kind(), ErrorKind::InvalidInput);
        assert_eq!(bootstrap_error.to_string(), config_error.to_string());
        for secret in ["private-user", "private-password", "private-proxy.example"] {
            assert!(!config_error.to_string().contains(secret));
        }
    }
    Ok(())
}

#[tokio::test]
async fn proxy_settings_follow_profile_and_cli_precedence() -> std::io::Result<()> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        "[proxy]\nurl = \"socks5h://base.example:1080\"\n",
    )?;
    let profile_path = codex_home.path().join("work.config.toml");
    std::fs::write(
        &profile_path,
        "[proxy]\nurl = \"socks5h://profile.example:1080\"\n",
    )?;
    let mut loader_overrides = LoaderOverrides::with_managed_config_path_for_tests(
        codex_home.path().join("managed_config.toml"),
    );
    loader_overrides.user_config_path = Some(AbsolutePathBuf::from_absolute_path(profile_path)?);
    loader_overrides.user_config_profile = Some("work".parse().expect("valid profile name"));
    for (cli_overrides, expected_url) in [
        (Vec::new(), "socks5h://profile.example:1080"),
        (
            vec![(
                "proxy.url".to_string(),
                TomlValue::String("socks5h://override.example:1080".to_string()),
            )],
            "socks5h://override.example:1080",
        ),
    ] {
        let config = ConfigBuilder::without_managed_config_for_tests()
            .codex_home(codex_home.path().to_path_buf())
            .loader_overrides(loader_overrides.clone())
            .cli_overrides(cli_overrides)
            .build()
            .await?;
        assert_eq!(
            config
                .http_client_factory()
                .resolve_proxy_route("https://api.openai.com/v1/responses"),
            OutboundProxyRoute::Proxy {
                url: expected_url.to_string(),
                no_proxy: None,
            }
        );
    }
    Ok(())
}
