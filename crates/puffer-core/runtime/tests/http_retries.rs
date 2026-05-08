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
fn http_retry_config_defaults_to_no_retries() {
    let _lock = super::refresh_env_lock()
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let _attempts = ScopedEnvVar::unset(super::super::HTTP_RETRY_ATTEMPTS_ENV);
    let _delay = ScopedEnvVar::unset(super::super::HTTP_RETRY_DELAY_MS_ENV);

    assert_eq!(
        super::super::http_retry_config(),
        super::super::HttpRetryConfig {
            retries: 0,
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
