use std::time::Duration;

struct ScopedEnvVar {
    name: &'static str,
    old_value: Option<String>,
}

impl ScopedEnvVar {
    fn set(name: &'static str, value: &str) -> Self {
        let old_value = std::env::var(name).ok();
        std::env::set_var(name, value);
        Self { name, old_value }
    }

    fn unset(name: &'static str) -> Self {
        let old_value = std::env::var(name).ok();
        std::env::remove_var(name);
        Self { name, old_value }
    }
}

impl Drop for ScopedEnvVar {
    fn drop(&mut self) {
        if let Some(value) = self.old_value.take() {
            std::env::set_var(self.name, value);
        } else {
            std::env::remove_var(self.name);
        }
    }
}

#[test]
fn http_retry_config_defaults_to_three_retries() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _attempts = ScopedEnvVar::unset(super::super::HTTP_RETRY_ATTEMPTS_ENV);
    let _delay = ScopedEnvVar::unset(super::super::HTTP_RETRY_DELAY_MS_ENV);

    assert_eq!(
        super::super::http_retry_config(),
        super::super::HttpRetryConfig {
            retries: 3,
            delay_ms: 1_000,
        }
    );
}

#[test]
fn http_retry_config_reads_and_clamps_env_values() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _attempts = ScopedEnvVar::set(super::super::HTTP_RETRY_ATTEMPTS_ENV, "99");
    let _delay = ScopedEnvVar::set(super::super::HTTP_RETRY_DELAY_MS_ENV, "999999");

    assert_eq!(
        super::super::http_retry_config(),
        super::super::HttpRetryConfig {
            retries: 10,
            delay_ms: 30_000,
        }
    );
}

#[test]
fn retry_delay_scales_with_attempt_number() {
    let config = super::super::HttpRetryConfig {
        retries: 5,
        delay_ms: 250,
    };

    assert_eq!(
        super::super::retry_delay(config, 3),
        Duration::from_millis(750)
    );
}

#[test]
fn retryable_http_error_accepts_timeout_io_errors() {
    let error = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "timed out",
    ));

    assert!(super::super::is_retryable_http_error(&error));
}

#[test]
fn retryable_http_error_rejects_invalid_data_io_errors() {
    let error = anyhow::Error::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "bad payload",
    ));

    assert!(!super::super::is_retryable_http_error(&error));
}

#[test]
fn raw_http_request_uses_configured_proxy_for_provider_url() {
    let proxy = puffer_config::ProxyConfig {
        enabled: true,
        selected: Some("local".to_string()),
        bypass: vec![],
        proxies: vec![puffer_config::ProxyEndpoint {
            id: "local".to_string(),
            scheme: puffer_config::ProxyScheme::Http,
            host: "127.0.0.1".to_string(),
            port: 9,
            username: None,
            password: None,
        }],
    };
    let result = super::super::send_http_request_raw_with_proxy(
        "https://api.openai.com/v1/responses",
        &[],
        "{}",
        false,
        &proxy,
    );
    assert!(result.is_err());
}

#[test]
fn http_5xx_max_attempts_defaults_to_three() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = ScopedEnvVar::unset("PUFFER_HTTP_5XX_MAX_ATTEMPTS");
    assert_eq!(super::super::http_5xx_max_attempts(), 3);
}

#[test]
fn http_5xx_max_attempts_clamps_to_one_to_five() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    {
        let _env = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "0");
        assert_eq!(super::super::http_5xx_max_attempts(), 1);
    }
    {
        let _env = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "99");
        assert_eq!(super::super::http_5xx_max_attempts(), 5);
    }
    {
        let _env = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "not-a-number");
        assert_eq!(super::super::http_5xx_max_attempts(), 3);
    }
}

#[test]
fn http_5xx_base_delay_defaults_to_500ms() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _env = ScopedEnvVar::unset("PUFFER_HTTP_5XX_BASE_DELAY_MS");
    assert_eq!(
        super::super::http_5xx_base_delay(),
        Duration::from_millis(500)
    );
}

#[test]
fn http_5xx_base_delay_clamps_extreme_values() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    {
        let _env = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "10");
        assert_eq!(
            super::super::http_5xx_base_delay(),
            Duration::from_millis(100)
        );
    }
    {
        let _env = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "120000");
        assert_eq!(
            super::super::http_5xx_base_delay(),
            Duration::from_millis(8_000)
        );
    }
}

#[test]
fn http_5xx_backoff_grows_exponentially_within_cap() {
    let base = Duration::from_millis(1_000);
    // attempt=1 → base * 2^0 = 1000ms (minus jitter, so ≤ 1000ms)
    let d1 = super::super::http_5xx_backoff_with_jitter(base, 1);
    assert!(d1 <= Duration::from_millis(1_000));
    assert!(d1 >= Duration::from_millis(750)); // ≤ 25% jitter

    // attempt=2 → base * 2 = 2000ms (minus jitter)
    let d2 = super::super::http_5xx_backoff_with_jitter(base, 2);
    assert!(d2 <= Duration::from_millis(2_000));
    assert!(d2 >= Duration::from_millis(1_500));

    // attempt=10 → capped at 8000ms (minus jitter)
    let d_huge = super::super::http_5xx_backoff_with_jitter(base, 10);
    assert!(d_huge <= Duration::from_millis(8_000));
    assert!(d_huge >= Duration::from_millis(6_000));
}

#[test]
fn parse_retry_after_ms_header_takes_precedence() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after-ms", "1500".parse().unwrap());
    headers.insert("retry-after", "30".parse().unwrap()); // would be 30s
    let parsed = super::super::parse_retry_after_headers(&headers);
    assert_eq!(parsed, Some(Duration::from_millis(1_500)));
}

#[test]
fn parse_retry_after_seconds_integer() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", "5".parse().unwrap());
    let parsed = super::super::parse_retry_after_headers(&headers);
    assert_eq!(parsed, Some(Duration::from_millis(5_000)));
}

#[test]
fn parse_retry_after_caps_at_60s() {
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", "9999".parse().unwrap());
    let parsed = super::super::parse_retry_after_headers(&headers);
    assert_eq!(parsed, Some(Duration::from_millis(60_000)));

    let mut headers_ms = reqwest::header::HeaderMap::new();
    headers_ms.insert("retry-after-ms", "12345678".parse().unwrap());
    let parsed_ms = super::super::parse_retry_after_headers(&headers_ms);
    assert_eq!(parsed_ms, Some(Duration::from_millis(60_000)));
}

#[test]
fn parse_retry_after_http_date_in_future() {
    // Build an HTTP-date ~3 seconds in the future and check we get
    // approximately 3000ms back. RFC 2822 covers the IMF-fixdate
    // format (`Sun, 06 Nov 1994 08:49:37 GMT`).
    let target = time::OffsetDateTime::now_utc() + time::Duration::seconds(3);
    let formatted = target
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", formatted.parse().unwrap());
    let parsed = super::super::parse_retry_after_headers(&headers).unwrap();
    // Allow 1s slop for clock drift between format/parse.
    assert!(
        parsed >= Duration::from_millis(1_000) && parsed <= Duration::from_millis(4_000),
        "expected ~3s, got {parsed:?}"
    );
}

#[test]
fn parse_retry_after_http_date_in_past_returns_zero() {
    let target = time::OffsetDateTime::now_utc() - time::Duration::seconds(60);
    let formatted = target
        .format(&time::format_description::well_known::Rfc2822)
        .unwrap();
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert("retry-after", formatted.parse().unwrap());
    let parsed = super::super::parse_retry_after_headers(&headers);
    assert_eq!(parsed, Some(Duration::ZERO));
}

#[test]
fn parse_retry_after_returns_none_when_absent_or_invalid() {
    let empty = reqwest::header::HeaderMap::new();
    assert_eq!(super::super::parse_retry_after_headers(&empty), None);

    let mut bad = reqwest::header::HeaderMap::new();
    bad.insert("retry-after", "not-a-number-or-date".parse().unwrap());
    assert_eq!(super::super::parse_retry_after_headers(&bad), None);
}

#[test]
fn retry_on_5xx_honors_retry_after_seconds() {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    // Set base delay much smaller than the Retry-After value so that
    // observing >=900ms can only come from the header, not backoff.
    let _delay = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "100");
    let _max = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "2");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_thread = counter.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let attempt = counter_thread.fetch_add(1, Ordering::SeqCst);
                let response_bytes: Vec<u8> = if attempt == 0 {
                    b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 7\r\n\r\nofflinE".to_vec()
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
                };
                let _ = stream.write_all(&response_bytes);
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    let retries = RefCell::new(0_usize);
    let started = Instant::now();
    let response = super::super::retry_on_5xx(
        || client.get(&url).send().map_err(anyhow::Error::from),
        |_, _, _| {
            *retries.borrow_mut() += 1;
        },
    )
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(*retries.borrow(), 1);
    assert!(
        elapsed >= Duration::from_millis(900),
        "expected ~1s delay from Retry-After, got {elapsed:?}"
    );
    // Sanity bound: shouldn't be massively over 1s either.
    assert!(
        elapsed < Duration::from_millis(3_000),
        "delay should be bounded, got {elapsed:?}"
    );
    drop(server);
}

#[test]
fn retry_on_5xx_honors_retry_after_ms_header() {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    // Base would normally be 100ms — we want 1500ms specifically from
    // the header to prove `retry-after-ms` is being read.
    let _delay = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "100");
    let _max = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "2");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_thread = counter.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let attempt = counter_thread.fetch_add(1, Ordering::SeqCst);
                let response_bytes: Vec<u8> = if attempt == 0 {
                    b"HTTP/1.1 503 Service Unavailable\r\nretry-after-ms: 1500\r\nContent-Length: 7\r\n\r\nofflinE".to_vec()
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
                };
                let _ = stream.write_all(&response_bytes);
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    let retries = RefCell::new(0_usize);
    let started = Instant::now();
    let response = super::super::retry_on_5xx(
        || client.get(&url).send().map_err(anyhow::Error::from),
        |_, _, _| {
            *retries.borrow_mut() += 1;
        },
    )
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(*retries.borrow(), 1);
    assert!(
        elapsed >= Duration::from_millis(1_400),
        "expected ~1.5s delay from retry-after-ms, got {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_millis(3_500),
        "delay should be bounded, got {elapsed:?}"
    );
    drop(server);
}

#[test]
fn retry_on_5xx_falls_back_to_backoff_without_retry_after() {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;

    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    // Base 200ms → attempt=1 yields between 150ms and 200ms.
    // We assert the elapsed delay is close to 200ms (NOT 1s+), which
    // proves we did NOT apply a Retry-After header (none was sent).
    let _delay = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "200");
    let _max = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "2");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_thread = counter.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let attempt = counter_thread.fetch_add(1, Ordering::SeqCst);
                let response_bytes: Vec<u8> = if attempt == 0 {
                    // No Retry-After header on purpose.
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 7\r\n\r\nofflinE".to_vec()
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
                };
                let _ = stream.write_all(&response_bytes);
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    let retries = RefCell::new(0_usize);
    let started = Instant::now();
    let response = super::super::retry_on_5xx(
        || client.get(&url).send().map_err(anyhow::Error::from),
        |_, _, _| {
            *retries.borrow_mut() += 1;
        },
    )
    .unwrap();
    let elapsed = started.elapsed();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(*retries.borrow(), 1);
    // Backoff is 200ms - up to 25% jitter = [150ms, 200ms].
    // Generous upper bound to stay <1s (proves no Retry-After path).
    assert!(
        elapsed < Duration::from_millis(900),
        "no Retry-After → exponential backoff, expected <900ms, got {elapsed:?}"
    );
    drop(server);
}

#[test]
fn retry_on_5xx_returns_first_success() {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok");
            }
        }
    });
    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    let retries = RefCell::new(0_usize);
    let response = super::super::retry_on_5xx(
        || client.get(&url).send().map_err(anyhow::Error::from),
        |_, _, _| {
            *retries.borrow_mut() += 1;
        },
    )
    .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(*retries.borrow(), 0, "no retries on success");
    drop(server);
}

#[test]
fn retry_on_5xx_retries_then_succeeds() {
    use std::cell::RefCell;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _delay = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "100");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let counter = Arc::new(AtomicUsize::new(0));
    let counter_thread = counter.clone();
    let server = std::thread::spawn(move || {
        for _ in 0..3 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let attempt = counter_thread.fetch_add(1, Ordering::SeqCst);
                let response_bytes = if attempt < 2 {
                    b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 7\r\n\r\nofflinE".to_vec()
                } else {
                    b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok".to_vec()
                };
                let _ = stream.write_all(&response_bytes);
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    let retries = RefCell::new(0_usize);
    let response = super::super::retry_on_5xx(
        || client.get(&url).send().map_err(anyhow::Error::from),
        |_, _, _| {
            *retries.borrow_mut() += 1;
        },
    )
    .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert_eq!(
        *retries.borrow(),
        2,
        "two retries before the third attempt succeeded"
    );
    drop(server);
}

#[test]
fn retry_on_5xx_returns_final_5xx_when_exhausted() {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _delay = ScopedEnvVar::set("PUFFER_HTTP_5XX_BASE_DELAY_MS", "100");
    let _max = ScopedEnvVar::set("PUFFER_HTTP_5XX_MAX_ATTEMPTS", "2");

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        for _ in 0..2 {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 7\r\n\r\nofflinE");
            }
        }
    });

    let client = reqwest::blocking::Client::new();
    let url = format!("http://{addr}/");

    let response = super::super::retry_on_5xx(
        || client.get(&url).send().map_err(anyhow::Error::from),
        |_, _, _| {},
    )
    .unwrap();

    assert_eq!(response.status(), reqwest::StatusCode::BAD_GATEWAY);
    drop(server);
}
