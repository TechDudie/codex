use super::ProxyConfigToml;
use crate::config_toml::ConfigToml;
use crate::profile_toml::ConfigProfile;
use pretty_assertions::assert_eq;

#[test]
fn proxy_table_round_trips_in_root_and_profile_config() {
    let input = "[proxy]\nurl = \"socks5h://user:password@proxy.example:1080\"\n";
    let expected = Some(ProxyConfigToml {
        url: "socks5h://user:password@proxy.example:1080".to_string(),
    });
    let config: ConfigToml = toml::from_str(input).expect("valid proxy config");
    let profile: ConfigProfile = toml::from_str(input).expect("valid proxy profile");
    assert_eq!(config.proxy, expected);
    assert_eq!(profile.proxy, expected);
    let round_trip: ConfigToml =
        toml::from_str(&toml::to_string(&config).expect("serialize proxy config"))
            .expect("deserialize proxy config");
    assert_eq!(round_trip, config);
}

#[test]
fn proxy_table_requires_url() {
    let error = toml::from_str::<ConfigToml>("[proxy]\n").expect_err("missing proxy url");
    assert!(error.to_string().contains("missing field `url`"));
}

#[test]
fn proxy_debug_output_redacts_url_and_credentials() {
    let config: ConfigToml =
        toml::from_str("[proxy]\nurl = \"socks5h://user:password@proxy.example:1080\"\n")
            .expect("valid proxy config");
    assert_eq!(
        format!("{:?}", config.proxy.expect("configured proxy")),
        "ProxyConfigToml { url: \"<redacted>\" }"
    );
}
