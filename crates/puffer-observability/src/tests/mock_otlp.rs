//! Mock OTLP/HTTP collector that just stashes incoming protobuf
//! payloads in memory so tests can assert on them. We don't bother
//! decoding the protobuf — span attribute names show up as ASCII
//! strings inside the binary frame, so substring search is enough to
//! confirm we sent a span at all.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::{ObservabilityConfig, ObservabilityHandle};
use base64::Engine;
use opentelemetry::trace::{Span, Tracer};
use opentelemetry::KeyValue;
use std::sync::OnceLock;

/// All tests that touch process-global OTel state (`set_tracer_provider`)
/// or env vars share this lock so cargo's default parallel runner
/// can't race them.
fn global_test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

struct MockServer {
    addr: String,
    requests: Arc<Mutex<Vec<Vec<u8>>>>,
}

fn spawn_mock() -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
    listener.set_nonblocking(false).ok();
    let addr = format!("http://{}", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let collected = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut s) => {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut buf = vec![0u8; 64 * 1024];
                    let mut total = Vec::new();
                    loop {
                        match s.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                total.extend_from_slice(&buf[..n]);
                                // Crude: stop reading once we've seen
                                // the end of the body header section
                                // and at least 1 KB of body.
                                if total.windows(4).any(|w| w == b"\r\n\r\n") && total.len() > 256 {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                    collected.lock().unwrap().push(total);
                    let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                    let _ = s.write_all(resp);
                    let _ = s.flush();
                }
                Err(_) => break,
            }
        }
    });
    MockServer { addr, requests }
}

fn config_for(addr: &str) -> ObservabilityConfig {
    ObservabilityConfig {
        endpoint: addr.to_string(),
        service_name: "puffer-test".to_string(),
        headers: Vec::new(),
        sample_rate: 1.0,
        include_prompts: true,
        include_outputs: true,
        include_tool_io: true,
        ..ObservabilityConfig::default()
    }
}

#[test]
fn handle_emits_span_to_mock_otlp_collector() {
    let _guard = global_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let server = spawn_mock();
    let handle = ObservabilityHandle::init(config_for(&server.addr)).expect("init handle");
    {
        let tracer = handle.tracer();
        let mut span = tracer.start("agent_loop");
        span.set_attribute(KeyValue::new("puffer.session.id", "test-session"));
        span.set_attribute(KeyValue::new("puffer.cwd", "/tmp/pup"));
        // Drop ends the span; BatchSpanProcessor flushes on drop.
        drop(span);
    }
    handle.shutdown();
    // Give the BatchSpanProcessor up to ~5s to flush over the wire.
    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if !server.requests.lock().unwrap().is_empty() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let reqs = server.requests.lock().unwrap();
    assert!(!reqs.is_empty(), "mock collector did not receive any spans");
    let payload = &reqs[0];
    let as_str: String = payload.iter().map(|&b| b as char).collect();
    // The OTLP/proto frame is binary, but ASCII attribute keys appear
    // verbatim (length-prefixed strings).
    assert!(as_str.contains("agent_loop"), "span name not in payload");
    assert!(
        as_str.contains("puffer.session.id"),
        "puffer.session.id attribute not in payload"
    );
    assert!(
        as_str.contains("test-session"),
        "session value not in payload"
    );
    assert!(
        as_str.contains("puffer-test"),
        "service.name resource attribute not in payload"
    );
}

#[test]
fn try_init_from_env_returns_none_when_unconfigured() {
    let _guard = global_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    // Save / restore so we don't bleed state.
    let prev = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    let result = ObservabilityHandle::try_init_from_env().expect("env init");
    assert!(result.is_none());
    if let Some(p) = prev {
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", p);
    }
}

#[test]
fn try_init_from_env_returns_handle_when_endpoint_set() {
    let _guard = global_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let prev_endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT").ok();
    let prev_service = std::env::var("OTEL_SERVICE_NAME").ok();
    let server = spawn_mock();
    std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", &server.addr);
    std::env::set_var("OTEL_SERVICE_NAME", "puffer-env-test");
    let result = ObservabilityHandle::try_init_from_env().expect("env init");
    assert!(result.is_some());
    drop(result);
    if let Some(p) = prev_endpoint {
        std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", p);
    } else {
        std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
    }
    if let Some(p) = prev_service {
        std::env::set_var("OTEL_SERVICE_NAME", p);
    } else {
        std::env::remove_var("OTEL_SERVICE_NAME");
    }
}

#[test]
fn for_langfuse_builds_expected_endpoint_and_auth() {
    let cfg = ObservabilityConfig::for_langfuse("http://localhost:3000", "pk-lf-foo", "sk-lf-bar");
    assert_eq!(cfg.endpoint, "http://localhost:3000/api/public/otel");
    let auth = cfg
        .headers
        .iter()
        .find(|(k, _)| k == "Authorization")
        .map(|(_, v)| v.as_str())
        .expect("Authorization header");
    assert!(auth.starts_with("Basic "));
    let encoded = auth.trim_start_matches("Basic ");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .unwrap();
    assert_eq!(decoded, b"pk-lf-foo:sk-lf-bar");
    // Codex review note: pin the ingestion version so server-side
    // Langfuse upgrades don't silently route to a different parser.
    let ingest = cfg
        .headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("x-langfuse-ingestion-version"))
        .map(|(_, v)| v.as_str());
    assert_eq!(ingest, Some("4"));
}

#[test]
fn for_langfuse_default_does_not_send_content() {
    let cfg = ObservabilityConfig::for_langfuse("http://localhost:3000", "pk", "sk");
    assert!(!cfg.include_prompts);
    assert!(!cfg.include_outputs);
    assert!(!cfg.include_tool_io);
}

#[test]
fn for_langfuse_strips_trailing_slash() {
    let cfg = ObservabilityConfig::for_langfuse("http://localhost:3000/", "pk", "sk");
    assert_eq!(cfg.endpoint, "http://localhost:3000/api/public/otel");
}

/// Concurrent subagent emission. Three threads each open a fresh
/// agent_loop span (= a separate "subagent" trace), set attributes,
/// and drop. The mock OTLP collector should receive all of them
/// without losing or corrupting any. Reproduces the runtime path
/// where multiple `Agent` background tools or teammates run in
/// parallel and emit traces concurrently.
#[test]
fn concurrent_subagent_spans_all_arrive() {
    let _guard = global_test_lock().lock().unwrap_or_else(|p| p.into_inner());
    let server = spawn_mock();
    let handle = ObservabilityHandle::init(config_for(&server.addr)).expect("init handle");

    let workers = (0..3)
        .map(|i| {
            let handle = handle.clone();
            thread::spawn(move || {
                let tracer = handle.tracer();
                let mut span = tracer.start(format!("agent_loop_{i}"));
                span.set_attribute(KeyValue::new(
                    "puffer.session.id",
                    format!("sub-session-{i}"),
                ));
                span.set_attribute(KeyValue::new(
                    "puffer.parent.session_id",
                    "shared-parent-session",
                ));
                span.set_attribute(KeyValue::new("puffer.subagent.kind", "agent_tool"));
                drop(span);
            })
        })
        .collect::<Vec<_>>();
    for w in workers {
        w.join().expect("worker join");
    }
    handle.shutdown();

    let started = std::time::Instant::now();
    while started.elapsed() < Duration::from_secs(5) {
        if server.requests.lock().unwrap().len() >= 3 {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let reqs = server.requests.lock().unwrap();
    let combined: String = reqs
        .iter()
        .flat_map(|r| r.iter())
        .map(|&b| b as char)
        .collect();
    for i in 0..3 {
        assert!(
            combined.contains(&format!("agent_loop_{i}")),
            "missing concurrent span agent_loop_{i}: receive count = {}",
            reqs.len()
        );
        assert!(
            combined.contains(&format!("sub-session-{i}")),
            "missing per-subagent session_id sub-session-{i}"
        );
    }
    assert!(
        combined.matches("shared-parent-session").count() >= 3,
        "every concurrent subagent should carry the parent link; got {} occurrences",
        combined.matches("shared-parent-session").count()
    );
    assert!(
        combined.matches("puffer.subagent.kind").count() >= 3,
        "every concurrent subagent should carry the subagent.kind marker"
    );
}

#[test]
fn parse_headers_recognizes_common_shapes() {
    let kvs = crate::parse_headers_for_tests("Authorization=Bearer abc, X-Trace=on");
    assert_eq!(
        kvs,
        vec![
            ("Authorization".to_string(), "Bearer abc".to_string()),
            ("X-Trace".to_string(), "on".to_string()),
        ]
    );
}
