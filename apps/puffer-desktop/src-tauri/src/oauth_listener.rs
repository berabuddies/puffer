//! Loopback HTTP listener that receives the OAuth callback from the user's
//! system browser.
//!
//! Flow (mirrors the donor branch's puffer-cli pattern):
//!   1. Tauri startup binds 127.0.0.1:OAUTH_CALLBACK_PORT once for the
//!      lifetime of the app.
//!   2. The frontend's goToLogin sends `redirect_uri =
//!      http://localhost:OAUTH_CALLBACK_PORT/callback` to Auth Station
//!      and opens the auth URL via plugin-opener.
//!   3. Auth Station finishes login in the OS browser and 302s back to
//!      our loopback URL. The browser GETs us with `?token=…&state=…`.
//!   4. We read the request line, extract path+query, emit
//!      "oauth:callback" to the frontend with the full reconstructed URL.
//!   5. Respond with a tiny HTML body that closes the browser tab so the
//!      user lands back on the app without an orphan window.
//!
//! Why a fixed port: the redirect_uri must be on Auth Station's
//! ALLOWED_REDIRECT_ORIGINS allowlist, and that allowlist is checked
//! exactly. A fixed port (1457) lets us keep the whitelist entry stable.
//! Vite dev runs on 1456 so there's no conflict.
//!
//! Why we bind only loopback (127.0.0.1): no other machine on the LAN
//! should be able to deliver an auth callback to us.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

/// Fixed listener port. Must be present in Auth Station's
/// ALLOWED_REDIRECT_ORIGINS as `http://localhost:1457`.
pub const OAUTH_CALLBACK_PORT: u16 = 1457;

/// Tauri event name the frontend subscribes to.
pub const OAUTH_CALLBACK_EVENT: &str = "oauth:callback";

const SUCCESS_HTML: &str = concat!(
    "<!DOCTYPE html><html><head>",
    "<meta charset=\"utf-8\">",
    "<title>Signed in</title>",
    "<style>body{font-family:-apple-system,BlinkMacSystemFont,sans-serif;",
    "text-align:center;padding-top:4em;color:#333}h1{font-weight:500}</style>",
    "</head><body>",
    "<h1>You can close this tab</h1>",
    "<p>Returning to the app…</p>",
    "<script>setTimeout(function(){window.close()},150)</script>",
    "</body></html>"
);

const ERROR_HTML: &str = concat!(
    "<!DOCTYPE html><html><head><meta charset=\"utf-8\">",
    "<title>Sign-in error</title></head><body>",
    "<h1>Sign-in failed</h1><p>Please return to the app and try again.</p>",
    "</body></html>"
);

/// Spawn the OAuth callback listener on a background thread. Idempotent —
/// safe to call multiple times; subsequent calls fail to bind and log,
/// which is fine because the first one is still running.
pub fn start(handle: AppHandle) {
    thread::spawn(move || {
        let listener = match TcpListener::bind(("127.0.0.1", OAUTH_CALLBACK_PORT)) {
            Ok(listener) => listener,
            Err(err) => {
                eprintln!(
                    "[oauth-listener] failed to bind 127.0.0.1:{OAUTH_CALLBACK_PORT}: {err}"
                );
                return;
            }
        };
        // eprintln so it shows up alongside cargo's stdout during `tauri dev`.
        eprintln!("[oauth-listener] listening on http://127.0.0.1:{OAUTH_CALLBACK_PORT}");

        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                continue;
            };
            // Cap how long we wait on a slow client so a misbehaving
            // browser can't hold a thread forever.
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

            let mut buffer = [0u8; 4096];
            let request = match stream.read(&mut buffer) {
                Ok(0) => continue,
                Ok(n) => String::from_utf8_lossy(&buffer[..n]).to_string(),
                Err(err) => {
                    eprintln!("[oauth-listener] read error: {err}");
                    continue;
                }
            };

            // First line: "GET /callback?token=...&state=... HTTP/1.1"
            let path_and_query = request
                .lines()
                .next()
                .and_then(|line| line.split_whitespace().nth(1))
                .unwrap_or("/");

            let full_url = format!(
                "http://127.0.0.1:{OAUTH_CALLBACK_PORT}{path_and_query}"
            );

            // Ignore liveness pings / random crawler hits — we only want
            // /callback with a token or an error query string.
            let is_oauth_callback = path_and_query.starts_with("/callback")
                && (path_and_query.contains("token=") || path_and_query.contains("error"));

            if is_oauth_callback {
                if let Err(err) = handle.emit(OAUTH_CALLBACK_EVENT, &full_url) {
                    eprintln!("[oauth-listener] emit failed: {err}");
                }
                let _ = stream.write_all(http_response(SUCCESS_HTML).as_bytes());
            } else {
                let _ = stream.write_all(http_response(ERROR_HTML).as_bytes());
            }
        }
    });
}

fn http_response(body: &str) -> String {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}
