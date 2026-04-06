use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::process::Command;
use std::time::{Duration, Instant};

/// Owns one ephemeral localhost OAuth callback listener.
pub(crate) struct CallbackListener {
    listener: TcpListener,
    host: String,
    port: u16,
    expected_path: String,
    redirect_uri: String,
}

impl CallbackListener {
    /// Binds an OS-assigned localhost callback port for the provided path.
    pub(crate) fn bind_localhost(path: &str) -> Result<Self> {
        let listener = TcpListener::bind(("localhost", 0))
            .with_context(|| format!("failed to bind callback listener for {path}"))?;
        listener.set_nonblocking(true)?;
        let port = listener
            .local_addr()
            .context("failed to read callback listener address")?
            .port();
        Ok(Self {
            listener,
            host: "localhost".to_string(),
            port,
            expected_path: path.to_string(),
            redirect_uri: format!("http://localhost:{port}{path}"),
        })
    }

    /// Returns the automatic redirect URI associated with this listener.
    pub(crate) fn redirect_uri(&self) -> &str {
        &self.redirect_uri
    }

    /// Waits for a single callback request and returns the captured callback URL.
    pub(crate) fn wait_for_callback_url(&self, timeout: Duration) -> Result<Option<String>> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            match self.listener.accept() {
                Ok((mut stream, _)) => {
                    let mut buffer = [0_u8; 4096];
                    let bytes_read = stream.read(&mut buffer)?;
                    let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
                    if let Some(callback_url) =
                        parse_callback_request(&request, &self.host, self.port, &self.expected_path)
                    {
                        let _ = stream.write_all(success_response().as_bytes());
                        return Ok(Some(callback_url));
                    }
                    let _ = stream.write_all(error_response().as_bytes());
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }
}

/// Tries to open a URL in the user's default browser.
pub(crate) fn open_browser(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command.spawn().is_ok()
}

fn parse_callback_request(
    request: &str,
    host: &str,
    port: u16,
    expected_path: &str,
) -> Option<String> {
    let line = request.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    if method != "GET" {
        return None;
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != expected_path {
        return None;
    }
    let suffix = if query.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{query}")
    };
    Some(format!("http://{host}:{port}{suffix}"))
}

fn success_response() -> &'static str {
    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 74\r\n\r\n<html><body>Authentication completed. You can return to Puffer.</body></html>"
}

fn error_response() -> &'static str {
    "HTTP/1.1 400 Bad Request\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: 53\r\n\r\n<html><body>Invalid callback for Puffer.</body></html>"
}

#[cfg(test)]
mod tests {
    use super::parse_callback_request;
    use super::CallbackListener;
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

    #[test]
    fn parses_matching_get_request() {
        let request = "GET /auth/callback?code=abc&state=xyz HTTP/1.1\r\nHost: localhost\r\n\r\n";
        let parsed = parse_callback_request(request, "localhost", 1455, "/auth/callback");
        assert_eq!(
            parsed.as_deref(),
            Some("http://localhost:1455/auth/callback?code=abc&state=xyz")
        );
    }

    #[test]
    fn ignores_wrong_path() {
        let request = "GET /wrong HTTP/1.1\r\nHost: localhost\r\n\r\n";
        assert!(parse_callback_request(request, "localhost", 1455, "/auth/callback").is_none());
    }

    #[test]
    fn callback_listener_captures_matching_request() {
        let temp_listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        drop(temp_listener);

        let listener = CallbackListener::bind_localhost("/callback").unwrap();
        let redirect_uri = listener.redirect_uri().to_string();
        let handle = thread::spawn(move || {
            listener
                .wait_for_callback_url(Duration::from_secs(2))
                .unwrap()
        });

        thread::sleep(Duration::from_millis(150));
        let callback_port = url::Url::parse(&redirect_uri)
            .unwrap()
            .port_or_known_default()
            .unwrap();
        let mut stream = TcpStream::connect(("127.0.0.1", callback_port)).unwrap();
        stream
            .write_all(
                b"GET /callback?code=test-code&state=test-state HTTP/1.1\r\nHost: localhost\r\n\r\n",
            )
            .unwrap();
        stream.shutdown(Shutdown::Write).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.contains("Authentication completed"));

        let callback = handle.join().unwrap();
        let expected =
            format!("http://localhost:{callback_port}/callback?code=test-code&state=test-state");
        assert_eq!(callback.as_deref(), Some(expected.as_str()));
    }
}
