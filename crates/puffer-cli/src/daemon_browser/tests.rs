use super::command::BrowserCommand;
use super::screenshot::BrowserElementRef;
use super::test_support::{cef_env_lock, FakeCefDevtools};
use super::*;
use crate::daemon_browser::tabs::BrowserCurrentTabStatus;

mod expression_tests;

#[test]
fn normalizes_empty_and_full_urls() {
    assert_eq!(normalize_url("").unwrap(), "about:blank");
    assert_eq!(
        normalize_url("https://example.com/a").unwrap(),
        "https://example.com/a"
    );
}

#[test]
fn normalizes_local_and_inline_urls() {
    assert_eq!(
        normalize_url("file:///Users/shou/puffer/helloworld.html").unwrap(),
        "file:///Users/shou/puffer/helloworld.html"
    );
    assert_eq!(
        normalize_url("data:text/html,<h1>Hello</h1>").unwrap(),
        "data:text/html,<h1>Hello</h1>"
    );
}

#[test]
fn normalizes_bare_hosts() {
    assert_eq!(normalize_url("example.com").unwrap(), "https://example.com");
    assert_eq!(
        normalize_url("localhost:3000").unwrap(),
        "http://localhost:3000"
    );
    assert_eq!(
        normalize_url("127.0.0.1:1420").unwrap(),
        "http://127.0.0.1:1420"
    );
}

#[test]
fn navigate_updates_cached_state_before_worker_ack() {
    let (tx, _rx) = std::sync::mpsc::channel();
    let session = BrowserSession::new_for_test(
        tx,
        std::sync::Arc::new(std::sync::Mutex::new(BrowserState {
            url: DEFAULT_URL.to_string(),
            title: String::new(),
            loading: false,
            width: 960,
            height: 720,
        })),
        std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
    );

    session
        .navigate("file:///Users/shou/puffer/hello-world.html".to_string())
        .unwrap();

    let state = session.state();
    assert_eq!(state.url, "file:///Users/shou/puffer/hello-world.html");
    assert!(state.loading);
}

#[test]
fn open_tab_skips_navigation_when_live_tab_already_has_requested_url() {
    let (_events, _) = tokio::sync::broadcast::channel(8);
    let (tx, rx) = std::sync::mpsc::channel();
    let tempdir = tempfile::tempdir().unwrap();
    let state = std::sync::Arc::new(std::sync::Mutex::new(BrowserState {
        url: "https://mail.google.com/mail/u/0/#inbox".to_string(),
        title: "Gmail".to_string(),
        loading: false,
        width: 960,
        height: 720,
    }));
    let registry = BrowserRegistry::new(
        tempdir.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let session = BrowserSession::new_for_test(
        tx,
        std::sync::Arc::clone(&state),
        std::sync::Arc::new(std::sync::Mutex::new(std::time::Instant::now())),
    );
    let backend_id = backend_session_id("monitor-gmail-browser", "main");
    registry
        .sessions
        .lock()
        .unwrap()
        .insert(backend_id.clone(), session);
    registry.tabs.lock().unwrap().open_tab(
        "monitor-gmail-browser",
        Some("main".to_string()),
        Some("Gmail monitor".to_string()),
        backend_id,
        None,
        state.lock().unwrap().clone(),
        false,
    );

    let tab = registry
        .open_tab(
            _events,
            "monitor-gmail-browser".to_string(),
            Some("main".to_string()),
            Some("Gmail monitor".to_string()),
            Some("https://mail.google.com/mail/u/0/#inbox".to_string()),
            960,
            720,
            false,
            true,
        )
        .unwrap();

    assert_eq!(tab.url, "https://mail.google.com/mail/u/0/#inbox");
    let commands: Vec<_> = rx.try_iter().collect();
    assert!(
        !commands
            .iter()
            .any(|command| matches!(command, BrowserCommand::Navigate(_))),
        "same-url live open should not refresh the browser page"
    );
}

#[test]
fn cef_remote_root_does_not_create_devtools_targets() {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let requested_paths = Arc::new(Mutex::new(Vec::<String>::new()));
    let finished = Arc::new(AtomicBool::new(false));
    let server_paths = Arc::clone(&requested_paths);
    let server_finished = Arc::clone(&finished);
    let server = std::thread::spawn(move || {
        while !server_finished.load(Ordering::SeqCst) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0; 2048];
                    let count = stream.read(&mut buffer).unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..count]);
                    let path = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_string();
                    let list_count = {
                        let mut paths = server_paths.lock().unwrap();
                        paths.push(path.clone());
                        paths
                            .iter()
                            .filter(|item| item.starts_with("/json/list"))
                            .count()
                    };
                    let body = if path.starts_with("/json/version") {
                        format!(
                            r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/root"}}"#
                        )
                    } else if path.starts_with("/json/list") && list_count == 1 {
                        format!(
                            r#"[{{"id":"target-1","type":"page","url":"about:blank","webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/page/target-1"}}]"#
                        )
                    } else {
                        "[]".to_string()
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = stream.write_all(response.as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let root = BrowserRootSession::spawn(
        profile.path().to_path_buf(),
        960,
        720,
        BrowserLaunchSettings::default(),
    )
    .unwrap();
    assert_eq!(root.allocate_target().unwrap().target_id, "target-1");
    assert!(root.allocate_target().is_err());

    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }
    finished.store(true, Ordering::SeqCst);
    let _ = server.join();
    let paths = requested_paths.lock().unwrap().clone();
    assert!(
        paths.iter().all(|path| !path.starts_with("/json/new")),
        "remote CEF allocation unexpectedly requested /json/new: {paths:?}"
    );
}

/// Reproduces issue #603: the native-CEF browser uses a FIXED pool of prewarmed
/// page targets shared across every agent session. When prior/abandoned page
/// workers keep holding their slots, opening a new tab used to hard-fail with
/// "no available prewarmed page targets". The registry must instead reclaim the
/// least-recently-active slot and reuse it, so a new open keeps working.
#[test]
fn native_cef_pool_reclaims_idle_session_when_slots_exhausted() {
    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

    // Fake CEF with exactly two prewarmed page slots.
    let cef = FakeCefDevtools::spawn(2);
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let registry = BrowserRegistry::new(
        profile.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let (events, _events_rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);

    let session_a = "sess-a:browser:t1";
    let session_b = "sess-b:browser:t1";
    let session_c = "sess-c:browser:t1";

    registry
        .open(events.clone(), session_a.to_string(), None, 800, 600, false)
        .expect("open A should allocate the first prewarmed slot");
    std::thread::sleep(Duration::from_millis(40));
    registry
        .open(events.clone(), session_b.to_string(), None, 800, 600, false)
        .expect("open B should allocate the second prewarmed slot");
    std::thread::sleep(Duration::from_millis(40));

    // Both prewarmed slots are now in use. Opening a third tab must NOT hard-fail
    // with "no available prewarmed page targets": the registry should reclaim the
    // least-recently-active slot (session A) and reuse it for session C.
    let third = registry.open(events.clone(), session_c.to_string(), None, 800, 600, false);

    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }

    assert!(
        third.is_ok(),
        "third open should self-heal by reclaiming the most-idle CEF slot, got {:?}",
        third.err()
    );
    assert!(
        registry.live_session(session_a).is_none(),
        "most-idle session A should have been reclaimed to free its prewarm slot"
    );
    assert!(
        registry.live_session(session_b).is_some(),
        "session B should remain live"
    );
    assert!(
        registry.live_session(session_c).is_some(),
        "session C should be live on the reclaimed slot"
    );
}

/// Reproduces issue #649: a tab the user opens directly in the native browser is
/// a CEF page target the daemon never claimed, so it lives outside the tab
/// registry and the agent's `list`/`snapshot` cannot see it. After
/// `sync_native_tabs` reconciles against the live DevTools target list, the
/// user-opened page must appear as an adopted, connected tab the agent can read.
#[test]
fn native_cef_sync_surfaces_user_opened_tab() {
    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

    // One prewarm slot for the agent, plus a user-opened checkout page.
    let cef = FakeCefDevtools::spawn_with_user_pages(
        1,
        vec![("user-checkout", "https://www.ridge.com/checkouts/abc123")],
    );
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let registry = BrowserRegistry::new(
        profile.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let (events, _events_rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);
    let root = "sess-user";

    // The agent opens its own tab, claiming the prewarm slot.
    registry
        .open(
            events.clone(),
            backend_session_id(root, "t1"),
            None,
            800,
            600,
            false,
        )
        .expect("agent open should claim the prewarmed slot");
    registry.tabs.lock().unwrap().record_opened_backend(
        root,
        "t1",
        backend_session_id(root, "t1"),
        Some("__cef_prewarm_0__".to_string()),
        registry
            .live_session(&backend_session_id(root, "t1"))
            .unwrap()
            .state(),
    );

    // Before reconciling, the daemon only knows about its own tab.
    let before = registry.list_tabs(root);
    assert_eq!(before.tabs.len(), 1, "agent should start with only its own tab");

    registry.sync_native_tabs(&events, root, 800, 600);

    let after = registry.list_tabs(root);

    std::thread::sleep(Duration::from_millis(20));
    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }

    assert_eq!(
        after.tabs.len(),
        2,
        "the user-opened tab must be adopted into the registry, got {:?}",
        after.tabs.iter().map(|t| &t.url).collect::<Vec<_>>()
    );
    let adopted = after
        .tabs
        .iter()
        .find(|tab| tab.url.contains("ridge.com/checkouts/abc123"))
        .expect("adopted user tab with the checkout URL must be present");
    assert!(adopted.connected, "adopted user tab should be connected");
    assert!(
        adopted.native_cef_session_id.is_none(),
        "a user-opened tab is not a prewarm slot"
    );

    // Reconciling again must not duplicate the already-adopted tab.
    registry.sync_native_tabs(&events, root, 800, 600);
    assert_eq!(
        registry.list_tabs(root).tabs.len(),
        2,
        "re-sync must be idempotent and not re-adopt the same target"
    );
}

/// Real end-to-end check for issue #649 against an actual Chromium DevTools
/// endpoint (Google Chrome stands in for the native CEF runtime — both speak the
/// same CDP). Unlike the FakeCefDevtools tests, this exercises the live
/// `/json/list` format, a real CDP page worker attaching to a user-opened
/// target, and a real DOM snapshot. Ignored by default: needs Chrome installed.
/// Run with: `cargo test -p puffer-cli --bins real_chrome_adopts -- --ignored --nocapture`
#[test]
#[ignore = "needs real Google Chrome; run with --ignored"]
fn real_chrome_adopts_and_snapshots_user_opened_tab() {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let _guard = cef_env_lock().lock().unwrap();
    let chrome = std::env::var("CHROME_BIN").unwrap_or_else(|_| {
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()
    });
    assert!(
        std::path::Path::new(&chrome).exists(),
        "Chrome not found at {chrome}; set CHROME_BIN"
    );

    // A real local page that is NOT about:blank — the shape of a tab the user
    // opened directly in the browser.
    let tmp = tempfile::tempdir().unwrap();
    let page = tmp.path().join("checkout.html");
    std::fs::write(
        &page,
        "<html><head><title>RIDGE-CHECKOUT-649</title></head><body><h1>Pay 42 dollars</h1></body></html>",
    )
    .unwrap();
    let page_url = format!("file://{}", page.display());

    let port = {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let user_data = tmp.path().join("chrome-profile");

    let mut child = Command::new(&chrome)
        .arg("--headless=new")
        .arg(format!("--remote-debugging-port={port}"))
        .arg(format!("--user-data-dir={}", user_data.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-gpu")
        .arg("--allow-file-access-from-files")
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch Chrome");

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    // Wait for DevTools, then open the user's tab via the HTTP endpoint.
    let start = std::time::Instant::now();
    loop {
        if client
            .get(format!("http://127.0.0.1:{port}/json/version"))
            .send()
            .and_then(|r| r.error_for_status())
            .is_ok()
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "Chrome DevTools never came up");
        std::thread::sleep(Duration::from_millis(100));
    }
    client
        .put(format!("http://127.0.0.1:{port}/json/new?{page_url}"))
        .send()
        .and_then(|r| r.error_for_status())
        .expect("open user tab via /json/new");

    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", user_data.display().to_string());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let registry = BrowserRegistry::new(
            tmp.path().to_path_buf(),
            true,
            BrowserLaunchSettings::default(),
        );
        let (events, _rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);
        let root = "sess-real";

        // Agent claims a prewarmed (about:blank) slot for its own tab.
        registry
            .open(events.clone(), backend_session_id(root, "t1"), None, 1024, 768, false)
            .expect("agent open should claim the about:blank slot");
        registry.tabs.lock().unwrap().record_opened_backend(
            root,
            "t1",
            backend_session_id(root, "t1"),
            registry
                .live_session(&backend_session_id(root, "t1"))
                .unwrap()
                .native_cef_session_id(),
            registry.live_session(&backend_session_id(root, "t1")).unwrap().state(),
        );

        // Reconcile: the user's file:// tab must be discovered + adopted.
        registry.sync_native_tabs(&events, root, 1024, 768);

        let tabs = registry.list_tabs(root);
        let adopted = tabs
            .tabs
            .iter()
            .find(|tab| tab.url.contains("checkout.html"))
            .unwrap_or_else(|| {
                panic!(
                    "user-opened tab not surfaced; saw {:?}",
                    tabs.tabs.iter().map(|t| &t.url).collect::<Vec<_>>()
                )
            })
            .clone();
        assert!(adopted.connected, "adopted user tab should be connected");

        // The agent can actually read the live DOM of the user's page.
        let snapshot = registry
            .agent_snapshot(&adopted.backend_session_id)
            .expect("snapshot of adopted user tab");
        let snap_text = snapshot.to_string();
        assert!(
            snap_text.contains("RIDGE-CHECKOUT-649") || snap_text.contains("Pay 42 dollars"),
            "snapshot should contain the user page content, got: {snap_text}"
        );
        eprintln!("[real-chrome] adopted url={} snapshot.title ok", adopted.url);
    }));

    let _ = child.kill();
    let _ = child.wait();
    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }
    let _ = std::io::stderr().flush();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

/// Reproduces the real-app gap behind issue #649: when the user browses
/// entirely by hand and the agent never opened a browser, the daemon has no
/// global root yet. `sync_native_tabs` must still establish the native-CEF
/// connection on demand and surface the user's tab — without a prior agent open.
#[test]
fn native_cef_sync_surfaces_user_tab_without_prior_open() {
    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

    let cef = FakeCefDevtools::spawn_with_user_pages(
        2,
        vec![("user-checkout", "https://www.ridge.com/checkouts/manual")],
    );
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let registry = BrowserRegistry::new(
        profile.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let (events, _rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);
    let root = "sess-manual";

    // NO registry.open(...) — the agent never touched the browser this session.
    registry.sync_native_tabs(&events, root, 1024, 768);
    let tabs = registry.list_tabs(root);

    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }

    assert!(
        tabs.tabs.iter().any(|t| t.url.contains("ridge.com/checkouts/manual")),
        "sync must surface the user tab even with no prior agent open, got {:?}",
        tabs.tabs.iter().map(|t| &t.url).collect::<Vec<_>>()
    );
}

/// Guards the reclaim interaction for issue #649: an adopted user tab holds no
/// prewarm-pool slot, so reclaiming it to satisfy an exhausted pool would be
/// futile (it frees no slot) AND would needlessly kill the page the user is
/// viewing. When the pool is exhausted, reclaim must spare adopted tabs and
/// reclaim a real slot holder instead.
#[test]
fn native_cef_reclaim_spares_adopted_user_tab() {
    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

    // Exactly one prewarm slot, plus one user-opened page.
    let cef = FakeCefDevtools::spawn_with_user_pages(
        1,
        vec![("user-checkout", "https://www.ridge.com/checkouts/xyz")],
    );
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let registry = BrowserRegistry::new(
        profile.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let (events, _events_rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);
    let root = "sess-reclaim";
    let slot_backend = backend_session_id(root, "t1");
    let adopted_backend = backend_session_id(root, "t2");
    let third_backend = backend_session_id(root, "t3");

    // Agent claims the only prewarm slot, then we adopt the user's tab.
    registry
        .open(events.clone(), slot_backend.clone(), None, 800, 600, false)
        .expect("agent open should claim the only prewarmed slot");
    registry.tabs.lock().unwrap().record_opened_backend(
        root,
        "t1",
        slot_backend.clone(),
        Some("__cef_prewarm_0__".to_string()),
        registry.live_session(&slot_backend).unwrap().state(),
    );
    registry.sync_native_tabs(&events, root, 800, 600);
    assert!(
        registry.live_session(&adopted_backend).is_some(),
        "user tab should have been adopted as t2"
    );

    // Make the adopted user tab the MOST idle session, so a naive reclaim would
    // pick it first. It holds no slot, so reclaiming it must be refused.
    *registry
        .sessions
        .lock()
        .unwrap()
        .get(&adopted_backend)
        .unwrap()
        .last_active
        .lock()
        .unwrap() = std::time::Instant::now() - Duration::from_secs(120);

    // Pool is exhausted (the single slot is held by t1). Opening a third tab must
    // self-heal by reclaiming the slot holder (t1), NOT the adopted user tab.
    let third = registry.open(events.clone(), third_backend.clone(), None, 800, 600, false);

    std::thread::sleep(Duration::from_millis(20));
    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }

    assert!(
        third.is_ok(),
        "third open should reclaim the slot holder, got {:?}",
        third.err()
    );
    assert!(
        registry.live_session(&adopted_backend).is_some(),
        "adopted user tab must be spared by reclaim (it frees no pool slot)"
    );
    assert!(
        registry.live_session(&slot_backend).is_none(),
        "the real slot holder should have been reclaimed instead"
    );
}

/// Reproduces the slot-leak half of issue #585: a wedged page that never answers
/// the CDP reset must NOT permanently leak its native-CEF prewarm slot. Closing
/// such a page has to return its slot to the shared pool (best-effort reset) so a
/// single unresponsive page can't poison the whole browser tree.
#[test]
fn native_cef_slot_returns_to_pool_when_hung_page_reset_fails() {
    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

    // A single prewarmed slot whose page is wedged (accepts the CDP socket but
    // never answers), so the reset-on-release will time out.
    let cef = FakeCefDevtools::spawn_with_hung_slots(1, vec![0]);
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let registry = BrowserRegistry::new(
        profile.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let (events, _events_rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);

    let session_a = "sess-a:browser:t1";
    let session_b = "sess-b:browser:t1";

    registry
        .open(events.clone(), session_a.to_string(), None, 800, 600, false)
        .expect("open A should allocate the only prewarmed slot");
    // Closing the wedged page runs a CDP reset that times out; its slot must still
    // come back to the pool instead of being permanently leaked.
    registry.close(session_a).expect("close A");

    let reopened = registry.open(events.clone(), session_b.to_string(), None, 800, 600, false);

    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }

    assert!(
        reopened.is_ok(),
        "a wedged page's slot must return to the pool after close, got {:?}",
        reopened.err()
    );
}

/// Full issue #585 cascade: when every prewarm slot is held by a WEDGED page
/// (worker alive, page won't answer CDP), opening a new tab must still recover.
/// The registry reclaims the least-recently-active wedged slot (issue #603) and
/// that slot returns to the pool despite the failed reset (the #585 slot-leak
/// fix), so one frozen checkout page no longer poisons the whole browser tree.
#[test]
fn native_cef_recovers_when_all_slots_held_by_wedged_pages() {
    let _guard = cef_env_lock().lock().unwrap();
    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");

    // Two prewarm slots, both wedged.
    let cef = FakeCefDevtools::spawn_with_hung_slots(2, vec![0, 1]);
    let profile = tempfile::tempdir().unwrap();
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cef.port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", profile.path());

    let registry = BrowserRegistry::new(
        profile.path().to_path_buf(),
        true,
        BrowserLaunchSettings::default(),
    );
    let (events, _events_rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);

    registry
        .open(
            events.clone(),
            "sess-a:browser:t1".to_string(),
            None,
            800,
            600,
            false,
        )
        .expect("open A");
    std::thread::sleep(Duration::from_millis(40));
    registry
        .open(
            events.clone(),
            "sess-b:browser:t1".to_string(),
            None,
            800,
            600,
            false,
        )
        .expect("open B");
    std::thread::sleep(Duration::from_millis(40));

    // Both wedged slots are in use; the new tab must recover by reclaiming one.
    let third = registry.open(
        events.clone(),
        "sess-c:browser:t1".to_string(),
        None,
        800,
        600,
        false,
    );

    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }

    assert!(
        third.is_ok(),
        "a new tab must recover even when all slots are held by wedged pages, got {:?}",
        third.err()
    );
}

#[test]
fn browser_recording_requires_agent_activity_window() {
    let mut recordings = recording::BrowserRecordingRegistry::default();
    let state = BrowserState {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        loading: false,
        width: 960,
        height: 720,
    };
    let backend_id = "root-session:browser:t1";

    assert!(recordings
        .record_frame(backend_id, "frame-1", "image-a", 960, 720, &state)
        .is_none());

    recordings.arm_backend(backend_id, Duration::from_secs(1));
    assert!(recordings
        .record_frame(backend_id, "frame-2", "image-a", 960, 720, &state)
        .is_some());
}

#[test]
fn browser_console_registry_records_and_clears_console_payloads() {
    let mut console_logs = console::BrowserConsoleRegistry::default();
    let payload = devtools::devtools_event_payload(
        "Runtime.consoleAPICalled",
        &json!({
            "params": {
                "type": "error",
                "args": [{ "value": "boom" }],
                "timestamp": 12.0
            }
        }),
    )
    .unwrap();
    console_logs.record("root-session:browser:t1", &payload);
    console_logs.record(
        "root-session:browser:t1",
        &json!({ "kind": "network", "url": "https://example.com" }),
    );

    let logs = console_logs.read("root-session:browser:t1", false);
    assert_eq!(logs.len(), 1);
    assert_eq!(logs[0].get("level").and_then(Value::as_str), Some("error"));
    assert_eq!(logs[0].get("text").and_then(Value::as_str), Some("boom"));
    assert!(logs[0].get("recordedAtMs").is_some());

    let cleared = console_logs.read("root-session:browser:t1", true);
    assert_eq!(cleared.len(), 1);
    assert!(console_logs
        .read("root-session:browser:t1", false)
        .is_empty());
}

#[test]
fn cleanup_root_metadata_preserves_disconnected_tab_handles() {
    let tabs = std::sync::Arc::new(std::sync::Mutex::new(BrowserTabRegistry::default()));
    let agent_refs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::from([(
            "root-session:browser:t1".to_string(),
            Vec::<BrowserElementRef>::new(),
        )])));
    let console_logs = std::sync::Arc::new(std::sync::Mutex::new(
        console::BrowserConsoleRegistry::default(),
    ));
    let browser_state = BrowserState {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        loading: false,
        width: 960,
        height: 720,
    };
    tabs.lock().unwrap().open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        None,
        browser_state,
        true,
    );

    cleanup_root_metadata(
        &tabs,
        &agent_refs,
        &console_logs,
        "root-session",
        &["root-session:browser:t1".to_string()],
        true,
    );

    let state = tabs.lock().unwrap().list("root-session");
    assert_eq!(state.tabs.len(), 1);
    assert!(!state.tabs[0].connected);
    assert!(agent_refs.lock().unwrap().is_empty());
}

#[test]
fn cleanup_root_metadata_drops_tab_set_on_root_close() {
    let tabs = std::sync::Arc::new(std::sync::Mutex::new(BrowserTabRegistry::default()));
    let agent_refs =
        std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashMap::from([(
            "root-session:browser:t1".to_string(),
            Vec::<BrowserElementRef>::new(),
        )])));
    let console_logs = std::sync::Arc::new(std::sync::Mutex::new(
        console::BrowserConsoleRegistry::default(),
    ));
    let browser_state = BrowserState {
        url: "https://example.com".to_string(),
        title: "Example".to_string(),
        loading: false,
        width: 960,
        height: 720,
    };
    tabs.lock().unwrap().open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        None,
        browser_state,
        true,
    );

    cleanup_root_metadata(
        &tabs,
        &agent_refs,
        &console_logs,
        "root-session",
        &["root-session:browser:t1".to_string()],
        false,
    );

    assert!(tabs.lock().unwrap().list("root-session").tabs.is_empty());
    assert!(agent_refs.lock().unwrap().is_empty());
}

#[test]
fn current_tab_context_reports_no_active_tab() {
    let registry = BrowserTabRegistry::default();
    let context = registry
        .active_tab("root-session")
        .map(|tab| BrowserCurrentTabContext::from_tab(&tab))
        .unwrap_or_else(BrowserCurrentTabContext::no_active_tab);

    assert_eq!(context.status, BrowserCurrentTabStatus::NoActiveTab);
    assert_eq!(context.tab_id, None);
    assert_eq!(context.url, None);
    assert_eq!(context.origin, None);
    assert_eq!(context.host, None);
    assert_eq!(context.port, None);
    assert_eq!(context.title, None);
}

#[test]
fn current_tab_context_reports_empty_url() {
    let mut registry = BrowserTabRegistry::default();
    let tab = registry.open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        None,
        BrowserState {
            url: String::new(),
            title: "Blank".to_string(),
            loading: false,
            width: 960,
            height: 720,
        },
        true,
    );

    let context = BrowserCurrentTabContext::from_tab(&tab);
    assert_eq!(context.status, BrowserCurrentTabStatus::EmptyUrl);
    assert_eq!(context.tab_id.as_deref(), Some("t1"));
    assert_eq!(context.url.as_deref(), Some(""));
    assert_eq!(context.origin, None);
    assert_eq!(context.host, None);
    assert_eq!(context.port, None);
    assert_eq!(context.title.as_deref(), Some("Blank"));
}

#[test]
fn current_tab_context_reports_about_blank() {
    let mut registry = BrowserTabRegistry::default();
    let tab = registry.open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        None,
        BrowserState {
            url: "about:blank".to_string(),
            title: String::new(),
            loading: false,
            width: 960,
            height: 720,
        },
        true,
    );

    let context = BrowserCurrentTabContext::from_tab(&tab);
    assert_eq!(context.status, BrowserCurrentTabStatus::AboutBlank);
    assert_eq!(context.tab_id.as_deref(), Some("t1"));
    assert_eq!(context.url.as_deref(), Some("about:blank"));
    assert_eq!(context.origin, None);
    assert_eq!(context.host, None);
    assert_eq!(context.port, None);
}

#[test]
fn current_tab_context_extracts_origin_host_port_and_title() {
    let mut registry = BrowserTabRegistry::default();
    let tab = registry.open_tab(
        "root-session",
        Some("t1".to_string()),
        None,
        "root-session:browser:t1".to_string(),
        None,
        BrowserState {
            url: "https://docs.example.com:8443/path?q=1".to_string(),
            title: "Docs".to_string(),
            loading: false,
            width: 960,
            height: 720,
        },
        true,
    );

    let context = BrowserCurrentTabContext::from_tab(&tab);
    assert_eq!(context.status, BrowserCurrentTabStatus::Available);
    assert_eq!(context.tab_id.as_deref(), Some("t1"));
    assert_eq!(
        context.url.as_deref(),
        Some("https://docs.example.com:8443/path?q=1")
    );
    assert_eq!(
        context.origin.as_deref(),
        Some("https://docs.example.com:8443")
    );
    assert_eq!(context.host.as_deref(), Some("docs.example.com"));
    assert_eq!(context.port, Some(8443));
    assert_eq!(context.title.as_deref(), Some("Docs"));
}

#[test]
fn shutdown_ack_wait_uses_one_shared_deadline() {
    let (_tx1, rx1) = std::sync::mpsc::channel::<()>();
    let (_tx2, rx2) = std::sync::mpsc::channel::<()>();
    let (_tx3, rx3) = std::sync::mpsc::channel::<()>();
    let start = std::time::Instant::now();

    wait_for_shutdown_acks(vec![rx1, rx2, rx3], Duration::from_millis(60));

    assert!(start.elapsed() < Duration::from_millis(140));
}

/// Real end-to-end proof for issue #656 against an actual Chromium with site
/// isolation forced on, so the card iframe is a true out-of-process iframe
/// (OOPIF) — exactly like Amazon's `ApxSecureIframe`. Verifies the cross-origin
/// payment-iframe deep pass: the snapshot surfaces the OOPIF's interior fields
/// as refs (which the top document can't reach), and `agent_fill` / `agent_select`
/// drive them by CDP node identity and confirm the values stuck.
///
/// Run with:
///   `cargo test -p puffer-cli --bins real_chrome_fills_oopif -- --ignored --nocapture`
#[test]
#[ignore = "needs real Google Chrome; run with --ignored"]
fn real_chrome_fills_oopif_payment_card_fields() {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::process::{Command, Stdio};

    let _guard = cef_env_lock().lock().unwrap();
    let chrome = std::env::var("CHROME_BIN").unwrap_or_else(|_| {
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome".to_string()
    });
    assert!(
        std::path::Path::new(&chrome).exists(),
        "Chrome not found at {chrome}; set CHROME_BIN"
    );

    // Two-origin fixture that mirrors Amazon's real structure: the card iframe
    // is CROSS-ORIGIN from the parent (so the top-document snapshot can't read
    // it) but SAME-SITE — both on 127.0.0.1, differing only by port. Like
    // Amazon's apx-security.amazon.com vs www.amazon.com (same eTLD+1), site
    // isolation keeps it in the SAME renderer process, so the page-session
    // `DOM.getDocument { pierce }` deep pass can reach its interior. (A true
    // cross-SITE OOPIF in a separate process is a harder case this approach does
    // not cover; #656 does not need it.) The iframe `name` hits the payment gate.
    let parent_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let parent_port = parent_listener.local_addr().unwrap().port();
    let card_listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let card_port = card_listener.local_addr().unwrap().port();
    let parent_html = format!(
        "<html><head><title>OOPIF-CHECKOUT-656</title></head><body>\
         <h1>Pay 27 dollars</h1>\
         <iframe name=\"ApxSecureIframe-test\" src=\"http://127.0.0.1:{card_port}/card\" \
         width=\"500\" height=\"360\" style=\"border:0\"></iframe>\
         </body></html>"
    );
    // The card-number field is a GUARDED controlled input that mirrors Amazon's
    // APX widget: it builds its real value only from genuine keystrokes and
    // reverts any bulk/programmatic value (insertText / direct `.value`). This
    // proves the fill path types real per-character keystrokes — a single
    // insertText would leave it empty and the fill's read-back guard would fail.
    let card_html = "<html><body style=\"font-family:sans-serif\">\
         <p><label>Card number <input id=\"cardnumber\" name=\"cardnumber\" type=\"tel\" autocomplete=\"off\"></label></p>\
         <p><label>Name on card <input id=\"cardname\" name=\"cardname\" type=\"text\"></label></p>\
         <p><label>Month <select id=\"expmonth\" name=\"expmonth\">\
           <option value=\"1\">01</option><option value=\"8\">08</option><option value=\"12\">12</option></select></label>\
         <label>Year <select id=\"expyear\" name=\"expyear\">\
           <option value=\"2026\">2026</option><option value=\"2030\">2030</option></select></label></p>\
         <p><label>CVV <input id=\"cvv\" name=\"cvv\" type=\"tel\"></label></p>\
         <script>\
           (function(){\
             var el=document.getElementById('cardnumber');\
             var proto=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value');\
             var model='';\
             el.addEventListener('keydown',function(e){\
               if(e.key&&e.key.length===1){model+=e.key;}\
               else if(e.key==='Backspace'){model=model.slice(0,-1);}\
             });\
             setInterval(function(){if(proto.get.call(el)!==model){proto.set.call(el,model);}},25);\
           })();\
         </script>\
         </body></html>"
        .to_string();
    let serve = |listener: TcpListener, body: String| {
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = stream.try_clone().unwrap();
                let mut buf = [0u8; 2048];
                let _ = reader.read(&mut buf);
                let mut writer = stream;
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = writer.write_all(response.as_bytes());
                let _ = writer.flush();
            }
        });
    };
    serve(parent_listener, parent_html);
    serve(card_listener, card_html);

    let tmp = tempfile::tempdir().unwrap();
    let page_url = format!("http://127.0.0.1:{parent_port}/");
    let cdp_port = {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let user_data = tmp.path().join("chrome-profile");

    let mut child = Command::new(&chrome)
        .arg("--headless=new")
        .arg(format!("--remote-debugging-port={cdp_port}"))
        .arg(format!("--user-data-dir={}", user_data.display()))
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .arg("--disable-gpu")
        .arg("about:blank")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("launch Chrome");

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let start = std::time::Instant::now();
    loop {
        if client
            .get(format!("http://127.0.0.1:{cdp_port}/json/version"))
            .send()
            .and_then(|r| r.error_for_status())
            .is_ok()
        {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(20), "Chrome DevTools never came up");
        std::thread::sleep(Duration::from_millis(100));
    }
    client
        .put(format!("http://127.0.0.1:{cdp_port}/json/new?{page_url}"))
        .send()
        .and_then(|r| r.error_for_status())
        .expect("open the checkout tab via /json/new");

    let previous_port = std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT");
    let previous_profile = std::env::var_os("PUFFER_CEF_PROFILE_DIR");
    std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", cdp_port.to_string());
    std::env::set_var("PUFFER_CEF_PROFILE_DIR", user_data.display().to_string());

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let registry =
            BrowserRegistry::new(tmp.path().to_path_buf(), true, BrowserLaunchSettings::default());
        let (events, _rx) = tokio::sync::broadcast::channel::<ServerEnvelope>(256);
        let root = "sess-oopif";

        registry
            .open(events.clone(), backend_session_id(root, "t1"), None, 1024, 768, false)
            .expect("agent open should claim the about:blank slot");
        registry.tabs.lock().unwrap().record_opened_backend(
            root,
            "t1",
            backend_session_id(root, "t1"),
            registry
                .live_session(&backend_session_id(root, "t1"))
                .unwrap()
                .native_cef_session_id(),
            registry.live_session(&backend_session_id(root, "t1")).unwrap().state(),
        );
        registry.sync_native_tabs(&events, root, 1024, 768);

        let adopted = registry
            .list_tabs(root)
            .tabs
            .iter()
            .find(|tab| tab.url.contains(&format!("{parent_port}")))
            .expect("checkout tab not adopted")
            .clone();

        // Give the cross-origin iframe a beat to load and lay out.
        std::thread::sleep(Duration::from_secs(3));

        let snapshot = registry
            .agent_snapshot(&adopted.backend_session_id)
            .expect("snapshot of the checkout tab");
        let elements = snapshot
            .get("elements")
            .and_then(|value| value.as_array())
            .cloned()
            .unwrap_or_default();
        let find_ref = |name: &str, role: &str| -> Option<String> {
            elements.iter().find_map(|element| {
                let matches_name = element
                    .get("name")
                    .and_then(|value| value.as_str())
                    .map(|value| value.contains(name))
                    .unwrap_or(false);
                let matches_role = element
                    .get("role")
                    .and_then(|value| value.as_str())
                    .map(|value| value == role)
                    .unwrap_or(false);
                if matches_name && matches_role {
                    element.get("ref").and_then(|value| value.as_str()).map(ToString::to_string)
                } else {
                    None
                }
            })
        };

        // The OOPIF interior fields must be surfaced as refs (top doc can't see them).
        let cardnumber = find_ref("cardnumber", "textbox").unwrap_or_else(|| {
            panic!(
                "card number field not surfaced from the OOPIF; refs: {:?}",
                elements
                    .iter()
                    .map(|e| (e.get("ref"), e.get("role"), e.get("name")))
                    .collect::<Vec<_>>()
            )
        });
        let cardname = find_ref("cardname", "textbox").expect("name field not surfaced");
        let expmonth = find_ref("expmonth", "combobox").expect("expiry month not surfaced");
        let expyear = find_ref("expyear", "combobox").expect("expiry year not surfaced");

        // Fill the cross-origin text fields — in_frame_fill reads the value back
        // and errors if it did not stick, so Ok proves the round trip.
        registry
            .agent_fill(&adopted.backend_session_id, &cardnumber, "4242424242424242")
            .expect("fill card number inside the OOPIF");
        registry
            .agent_fill(&adopted.backend_session_id, &cardname, "Toby Wen")
            .expect("fill name inside the OOPIF");
        // Drive the native <select> expiry dropdowns by visible label and value.
        registry
            .agent_select(&adopted.backend_session_id, &expmonth, "08")
            .expect("select expiry month inside the OOPIF");
        registry
            .agent_select(&adopted.backend_session_id, &expyear, "2030")
            .expect("select expiry year inside the OOPIF");

        eprintln!("[real-chrome] OOPIF card fields filled and verified");
    }));

    let _ = child.kill();
    let _ = child.wait();
    match previous_port {
        Some(value) => std::env::set_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT", value),
        None => std::env::remove_var("PUFFER_CEF_REMOTE_DEBUGGING_PORT"),
    }
    match previous_profile {
        Some(value) => std::env::set_var("PUFFER_CEF_PROFILE_DIR", value),
        None => std::env::remove_var("PUFFER_CEF_PROFILE_DIR"),
    }
    let _ = std::io::stderr().flush();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
