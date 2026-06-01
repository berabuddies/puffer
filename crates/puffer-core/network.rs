use anyhow::{Context, Result};
use puffer_config::{ProxyConfig, ProxyEndpoint};
use reqwest::blocking::Client;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Describes the outcome of a proxy connectivity request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProxyTestOutcome {
    /// Elapsed request time in milliseconds.
    pub latency_ms: u128,
    /// HTTP status observed from the connectivity target.
    pub status_code: u16,
}

/// Describes why an HTTP client is being constructed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpPurpose {
    Model,
    Discovery,
    OAuth,
    ConnectivityTest,
}

/// Builds a blocking reqwest client using the selected proxy when enabled.
pub fn blocking_client(
    proxy: &ProxyConfig,
    purpose: HttpPurpose,
    timeout: Duration,
) -> Result<Client> {
    let mut builder = Client::builder().timeout(timeout);
    if proxy.enabled {
        if let Some(endpoint) = selected_endpoint(proxy)? {
            builder = builder.proxy(reqwest::Proxy::all(proxy_uri(endpoint)?)?);
        }
    }
    let _ = purpose;
    builder.build().context("failed to build HTTP client")
}

/// Builds a blocking reqwest client for one target URL, honoring bypass entries.
pub fn blocking_client_for_url(
    proxy: &ProxyConfig,
    purpose: HttpPurpose,
    url: &str,
    timeout: Duration,
) -> Result<Client> {
    if proxy.enabled && !bypass_matches(proxy, url) {
        blocking_client(proxy, purpose, timeout)
    } else {
        Client::builder()
            .timeout(timeout)
            .build()
            .context("failed to build HTTP client")
    }
}

/// Tests a proxy endpoint against a URL and returns the observed response.
pub fn test_proxy_endpoint(
    endpoint: &ProxyEndpoint,
    target_url: &str,
    timeout: Duration,
) -> Result<ProxyTestOutcome> {
    let mut config = ProxyConfig::default();
    config.enabled = true;
    config.selected = Some(endpoint.id.clone());
    config.proxies = vec![endpoint.clone()];
    let started = Instant::now();
    let client =
        blocking_client_for_url(&config, HttpPurpose::ConnectivityTest, target_url, timeout)?;
    let response = client
        .get(target_url)
        .send()
        .with_context(|| format!("proxy test request to {target_url} failed"))?;
    Ok(ProxyTestOutcome {
        latency_ms: started.elapsed().as_millis(),
        status_code: response.status().as_u16(),
    })
}

/// Returns true when the URL host matches a configured bypass entry.
pub fn bypass_matches(proxy: &ProxyConfig, url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    let Some(host) = parsed.host_str() else {
        return false;
    };
    proxy
        .bypass
        .iter()
        .any(|entry| bypass_entry_matches(entry, host))
}

/// Builds the proxy URI accepted by reqwest.
pub fn proxy_uri(endpoint: &ProxyEndpoint) -> Result<String> {
    if endpoint.host.trim().is_empty() {
        anyhow::bail!("proxy host must not be empty");
    }
    let scheme = endpoint.scheme.as_uri_scheme();
    let host = endpoint.host.trim();
    let auth = match (
        endpoint
            .username
            .as_deref()
            .filter(|value| !value.is_empty()),
        endpoint
            .password
            .as_deref()
            .filter(|value| !value.is_empty()),
    ) {
        (Some(username), Some(password)) => format!(
            "{}:{}@",
            urlencoding::encode(username),
            urlencoding::encode(password)
        ),
        (Some(username), None) => format!("{}@", urlencoding::encode(username)),
        _ => String::new(),
    };
    Ok(format!("{scheme}://{auth}{host}:{}", endpoint.port))
}

fn selected_endpoint(proxy: &ProxyConfig) -> Result<Option<&ProxyEndpoint>> {
    let Some(selected) = proxy.selected.as_deref() else {
        return Ok(None);
    };
    proxy
        .proxies
        .iter()
        .find(|endpoint| endpoint.id == selected)
        .map(Some)
        .ok_or_else(|| anyhow::anyhow!("selected proxy `{selected}` does not exist"))
}

fn bypass_entry_matches(entry: &str, host: &str) -> bool {
    let entry = entry.trim();
    if entry.is_empty() {
        return false;
    }
    if entry.eq_ignore_ascii_case(host) {
        return true;
    }
    let Ok(host_ip) = host.parse::<IpAddr>() else {
        return false;
    };
    if let Ok(entry_ip) = entry.parse::<IpAddr>() {
        return host_ip == entry_ip;
    }
    if let Ok(net) = entry.parse::<ipnet::IpNet>() {
        return net.contains(&host_ip);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::ProxyScheme;

    fn proxy_config() -> ProxyConfig {
        ProxyConfig {
            enabled: true,
            selected: Some("local".to_string()),
            bypass: vec!["localhost".to_string(), "10.0.0.0/8".to_string()],
            proxies: vec![ProxyEndpoint {
                id: "local".to_string(),
                scheme: ProxyScheme::Socks5h,
                host: "127.0.0.1".to_string(),
                port: 7890,
                username: Some("user".to_string()),
                password: Some("pass".to_string()),
            }],
        }
    }

    #[test]
    fn proxy_uri_includes_encoded_credentials() {
        let endpoint = ProxyEndpoint {
            id: "auth".to_string(),
            scheme: ProxyScheme::Http,
            host: "proxy.example".to_string(),
            port: 8080,
            username: Some("user name".to_string()),
            password: Some("p@ss".to_string()),
        };
        assert_eq!(
            proxy_uri(&endpoint).expect("uri"),
            "http://user%20name:p%40ss@proxy.example:8080"
        );
    }

    #[test]
    fn bypass_matches_localhost_and_cidr() {
        let config = proxy_config();
        assert!(bypass_matches(&config, "http://localhost:3000/health"));
        assert!(bypass_matches(&config, "http://10.2.3.4/v1/models"));
        assert!(!bypass_matches(
            &config,
            "https://api.openai.com/v1/responses"
        ));
    }

    #[test]
    fn client_builder_accepts_selected_proxy() {
        let config = proxy_config();
        let client =
            blocking_client(&config, HttpPurpose::Model, Duration::from_secs(30)).expect("client");
        let _ = client;
    }

    #[test]
    fn proxy_test_treats_http_401_as_connected() {
        let (port, handle) = spawn_http_proxy_response("401 Unauthorized");
        let endpoint = ProxyEndpoint {
            id: "local".to_string(),
            scheme: ProxyScheme::Http,
            host: "127.0.0.1".to_string(),
            port,
            username: None,
            password: None,
        };

        let result = test_proxy_endpoint(
            &endpoint,
            "http://example.test/generate_204",
            Duration::from_secs(2),
        )
        .expect("HTTP response proves proxy connectivity");

        assert_eq!(result.status_code, 401);
        handle.join().expect("proxy thread");
    }

    fn spawn_http_proxy_response(status: &'static str) -> (u16, std::thread::JoinHandle<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind proxy");
        let port = listener.local_addr().expect("proxy address").port();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept proxy request");
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set read timeout");
            let mut request = Vec::new();
            let mut chunk = [0_u8; 1024];
            loop {
                let bytes =
                    std::io::Read::read(&mut stream, &mut chunk).expect("read proxy request");
                if bytes == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..bytes]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            let body = b"auth required";
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            std::io::Write::write_all(&mut stream, response.as_bytes())
                .expect("write proxy response headers");
            std::io::Write::write_all(&mut stream, body).expect("write proxy response body");
        });
        (port, handle)
    }
}
