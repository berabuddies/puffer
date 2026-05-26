use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DebugpyActionInput {
    action: String,
    port: u16,
}

/// Executes one debugpy DAP action backed by verified Lambda Skill contracts.
pub fn execute_debugpy_action(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: DebugpyActionInput =
        serde_json::from_value(input).context("invalid DebugpyAction input")?;
    match parsed.action.as_str() {
        "attach" => attach_debugpy(parsed.port),
        other => bail!("unsupported DebugpyAction action `{other}`"),
    }
}

fn attach_debugpy(port: u16) -> Result<String> {
    if port == 0 {
        bail!("DebugpyAction port must be between 1 and 65535");
    }
    let timeout = Duration::from_millis(DEFAULT_TIMEOUT_MS);
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = TcpStream::connect_timeout(&addr, timeout)
        .with_context(|| format!("connect to debugpy listener at 127.0.0.1:{port}"))?;
    stream
        .set_read_timeout(Some(timeout))
        .context("set debugpy read timeout")?;
    stream
        .set_write_timeout(Some(timeout))
        .context("set debugpy write timeout")?;

    let mut client = DapClient::new(stream);
    let initialize = client.send_request(
        "initialize",
        json!({
            "adapterID": "python",
            "clientID": "puffer",
            "clientName": "Puffer",
            "pathFormat": "path",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "supportsRunInTerminalRequest": false
        }),
    )?;
    client.expect_success(initialize, "initialize")?;
    let attach = client.send_request("attach", json!({}))?;
    let configuration_done = client.send_request("configurationDone", json!({}))?;
    client.expect_success(attach, "attach")?;
    client.expect_success(configuration_done, "configurationDone")?;
    Ok("null".to_string())
}

struct DapClient {
    stream: TcpStream,
    next_seq: u64,
    pending: Vec<Value>,
}

impl DapClient {
    fn new(stream: TcpStream) -> Self {
        Self {
            stream,
            next_seq: 1,
            pending: Vec::new(),
        }
    }

    fn send_request(&mut self, command: &str, arguments: Value) -> Result<u64> {
        let seq = self.next_seq;
        self.next_seq += 1;
        let payload = json!({
            "seq": seq,
            "type": "request",
            "command": command,
            "arguments": arguments
        });
        let body = serde_json::to_vec(&payload)?;
        write!(self.stream, "Content-Length: {}\r\n\r\n", body.len())
            .context("write DAP header")?;
        self.stream.write_all(&body).context("write DAP body")?;
        self.stream.flush().context("flush DAP request")?;
        Ok(seq)
    }

    fn expect_success(&mut self, request_seq: u64, command: &str) -> Result<()> {
        let response = self.read_response(request_seq, command)?;
        if response.get("success").and_then(Value::as_bool) == Some(true) {
            return Ok(());
        }
        let message = response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown DAP failure");
        bail!("DebugpyAction DAP `{command}` failed: {message}")
    }

    fn read_response(&mut self, request_seq: u64, command: &str) -> Result<Value> {
        if let Some(index) = self
            .pending
            .iter()
            .position(|message| matches_response(message, request_seq, command))
        {
            return Ok(self.pending.remove(index));
        }
        loop {
            let message = read_dap_message(&mut self.stream)?;
            if matches_response(&message, request_seq, command) {
                return Ok(message);
            }
            self.pending.push(message);
        }
    }
}

fn matches_response(message: &Value, request_seq: u64, command: &str) -> bool {
    message.get("type").and_then(Value::as_str) == Some("response")
        && message.get("request_seq").and_then(Value::as_u64) == Some(request_seq)
        && message.get("command").and_then(Value::as_str) == Some(command)
}

fn read_dap_message(stream: &mut TcpStream) -> Result<Value> {
    let mut header = Vec::new();
    let mut byte = [0u8; 1];
    while !header.ends_with(b"\r\n\r\n") {
        let read = stream.read(&mut byte).context("read DAP header")?;
        if read == 0 {
            bail!("debugpy closed connection before sending a DAP header");
        }
        header.push(byte[0]);
        if header.len() > 8192 {
            bail!("debugpy DAP header exceeded 8 KiB");
        }
    }
    let header = std::str::from_utf8(&header).context("DAP header was not UTF-8")?;
    let length = header
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .map(str::trim)
        .ok_or_else(|| anyhow!("DAP response missing Content-Length"))?
        .parse::<usize>()
        .context("parse DAP Content-Length")?;
    let mut body = vec![0u8; length];
    stream.read_exact(&mut body).context("read DAP body")?;
    serde_json::from_slice(&body).context("parse DAP body")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn attaches_to_mock_debugpy_adapter() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            for command in ["initialize", "attach", "configurationDone"] {
                let request = read_dap_message(&mut stream).unwrap();
                assert_eq!(
                    request.get("command").and_then(Value::as_str),
                    Some(command)
                );
                let request_seq = request.get("seq").and_then(Value::as_u64).unwrap();
                write_dap_response(&mut stream, request_seq, command);
            }
        });

        let output = attach_debugpy(port).unwrap();
        assert_eq!(output, "null");
        server.join().unwrap();
    }

    #[test]
    fn rejects_zero_port_before_network_use() {
        let error = attach_debugpy(0).unwrap_err();
        assert!(error.to_string().contains("port"));
    }

    fn write_dap_response(stream: &mut TcpStream, request_seq: u64, command: &str) {
        let body = serde_json::to_vec(&json!({
            "seq": request_seq + 100,
            "type": "response",
            "request_seq": request_seq,
            "success": true,
            "command": command
        }))
        .unwrap();
        write!(stream, "Content-Length: {}\r\n\r\n", body.len()).unwrap();
        stream.write_all(&body).unwrap();
    }
}
