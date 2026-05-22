use anyhow::{Context, Result};
use puffer_tools::internal_permissions::{
    InternalToolPermissionEnvelope, InternalToolPermissionRequest, InternalToolPermissionResponse,
    INTERNAL_PERMISSION_ADDR_ENV, INTERNAL_PERMISSION_PROTOCOL_VERSION,
    INTERNAL_PERMISSION_REQUIRED_ENV, INTERNAL_PERMISSION_TOKEN_ENV,
};
use std::io::{ErrorKind, Read as IoRead, Write as IoWrite};
use std::net::{TcpListener, TcpStream};
use uuid::Uuid;

const MAX_INTERNAL_PERMISSION_REQUEST_BYTES: usize = 64 * 1024;

pub(crate) type InternalPermissionHandler<'a> =
    dyn FnMut(InternalToolPermissionRequest) -> InternalToolPermissionResponse + 'a;

pub(crate) struct InternalPermissionBroker {
    listener: Option<TcpListener>,
    token: String,
    pending: Vec<PendingInternalPermissionRequest>,
}

struct PendingInternalPermissionRequest {
    stream: TcpStream,
    buffer: Vec<u8>,
}

enum PendingInternalPermissionOutcome {
    Keep,
    Remove,
}

impl InternalPermissionBroker {
    /// Starts a loopback internal tool permission broker when enabled.
    pub(crate) fn start(enabled: bool) -> Result<Self> {
        if !enabled {
            return Ok(Self {
                listener: None,
                token: String::new(),
                pending: Vec::new(),
            });
        };
        let listener =
            TcpListener::bind(("127.0.0.1", 0)).context("bind internal permission broker")?;
        listener
            .set_nonblocking(true)
            .context("configure internal permission broker")?;
        Ok(Self {
            listener: Some(listener),
            token: Uuid::new_v4().to_string(),
            pending: Vec::new(),
        })
    }

    /// Returns environment variables that connect child tools to this broker.
    pub(crate) fn envs(&self) -> Vec<(&'static str, String)> {
        let Some(listener) = &self.listener else {
            return Vec::new();
        };
        let Ok(addr) = listener.local_addr() else {
            return Vec::new();
        };
        vec![
            (INTERNAL_PERMISSION_ADDR_ENV, addr.to_string()),
            (INTERNAL_PERMISSION_TOKEN_ENV, self.token.clone()),
            (INTERNAL_PERMISSION_REQUIRED_ENV, "1".to_string()),
        ]
    }

    /// Handles pending permission connections without blocking Bash execution.
    pub(crate) fn drain_pending(
        &mut self,
        mut handler: Option<&mut InternalPermissionHandler<'_>>,
    ) -> Result<()> {
        if let Some(listener) = &self.listener {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        stream
                            .set_nonblocking(true)
                            .context("configure internal permission stream")?;
                        self.pending.push(PendingInternalPermissionRequest {
                            stream,
                            buffer: Vec::new(),
                        });
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                    Err(error) => return Err(error).context("accept internal permission request"),
                }
            }
        }

        let token = self.token.clone();
        let mut index = 0;
        while index < self.pending.len() {
            let outcome = poll_internal_permission_request(
                &token,
                &mut self.pending[index],
                handler.as_deref_mut(),
            )?;
            match outcome {
                PendingInternalPermissionOutcome::Keep => index += 1,
                PendingInternalPermissionOutcome::Remove => {
                    self.pending.swap_remove(index);
                }
            }
        }
        Ok(())
    }
}

fn poll_internal_permission_request(
    token: &str,
    pending: &mut PendingInternalPermissionRequest,
    handler: Option<&mut InternalPermissionHandler<'_>>,
) -> Result<PendingInternalPermissionOutcome> {
    let mut chunk = [0u8; 4096];
    loop {
        match pending.stream.read(&mut chunk) {
            Ok(0) if pending.buffer.is_empty() => {
                return Ok(PendingInternalPermissionOutcome::Remove)
            }
            Ok(0) => {
                let response =
                    InternalToolPermissionResponse::deny("incomplete internal permission request");
                write_internal_permission_response(&mut pending.stream, &response)?;
                return Ok(PendingInternalPermissionOutcome::Remove);
            }
            Ok(read) => {
                pending.buffer.extend_from_slice(&chunk[..read]);
                if pending.buffer.len() > MAX_INTERNAL_PERMISSION_REQUEST_BYTES {
                    let response = InternalToolPermissionResponse::deny(
                        "internal permission request is too large",
                    );
                    write_internal_permission_response(&mut pending.stream, &response)?;
                    return Ok(PendingInternalPermissionOutcome::Remove);
                }
                if let Some(newline) = pending.buffer.iter().position(|byte| *byte == b'\n') {
                    let mut line = pending.buffer[..newline].to_vec();
                    if line.ends_with(b"\r") {
                        line.pop();
                    }
                    let response =
                        match serde_json::from_slice::<InternalToolPermissionEnvelope>(&line) {
                            Ok(envelope) => {
                                resolve_internal_permission_envelope(token, envelope, handler)
                            }
                            Err(error) => InternalToolPermissionResponse::deny(format!(
                                "invalid internal permission request: {error}"
                            )),
                        };
                    write_internal_permission_response(&mut pending.stream, &response)?;
                    return Ok(PendingInternalPermissionOutcome::Remove);
                }
            }
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                return Ok(PendingInternalPermissionOutcome::Keep);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error).context("read internal permission request"),
        }
    }
}

fn resolve_internal_permission_envelope(
    token: &str,
    envelope: InternalToolPermissionEnvelope,
    handler: Option<&mut InternalPermissionHandler<'_>>,
) -> InternalToolPermissionResponse {
    if envelope.protocol_version != INTERNAL_PERMISSION_PROTOCOL_VERSION {
        return InternalToolPermissionResponse::deny("unsupported internal permission protocol");
    }
    if envelope.token != token {
        return InternalToolPermissionResponse::deny("invalid internal permission token");
    }
    let Some(handler) = handler else {
        return InternalToolPermissionResponse::deny("internal permission handler is unavailable");
    };
    handler(envelope.request)
}

fn write_internal_permission_response(
    stream: &mut TcpStream,
    response: &InternalToolPermissionResponse,
) -> Result<()> {
    stream
        .set_nonblocking(false)
        .context("configure internal permission response stream")?;
    let serialized = serde_json::to_string(response)?;
    stream
        .write_all(serialized.as_bytes())
        .context("write internal permission response")?;
    stream
        .write_all(b"\n")
        .context("finish internal permission response")?;
    stream.flush().context("flush internal permission response")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn internal_permission_broker_env_marks_permission_required() {
        let broker = InternalPermissionBroker::start(true).unwrap();
        let envs = broker.envs();

        assert!(envs
            .iter()
            .any(|(key, value)| *key == INTERNAL_PERMISSION_ADDR_ENV && !value.is_empty()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == INTERNAL_PERMISSION_TOKEN_ENV && !value.is_empty()));
        assert!(envs
            .iter()
            .any(|(key, value)| *key == INTERNAL_PERMISSION_REQUIRED_ENV && value == "1"));
    }

    #[test]
    fn internal_permission_envelope_delegates_structured_request() {
        let mut saw_request = false;
        let mut handler = |request: InternalToolPermissionRequest| {
            saw_request = true;
            assert_eq!(request.tool_id, "browser");
            assert_eq!(request.input, json!({"action":"snapshot"}));
            InternalToolPermissionResponse::allow()
        };

        let response = resolve_internal_permission_envelope(
            "token",
            InternalToolPermissionEnvelope {
                protocol_version: INTERNAL_PERMISSION_PROTOCOL_VERSION.to_string(),
                token: "token".to_string(),
                request: InternalToolPermissionRequest {
                    tool_id: "browser".to_string(),
                    input: json!({"action":"snapshot"}),
                },
            },
            Some(&mut handler),
        );

        assert!(response.is_allowed());
        assert!(saw_request);
    }

    #[test]
    fn internal_permission_envelope_rejects_wrong_token() {
        let mut handler = |_request: InternalToolPermissionRequest| {
            panic!("handler should not run for wrong token")
        };

        let response = resolve_internal_permission_envelope(
            "token",
            InternalToolPermissionEnvelope {
                protocol_version: INTERNAL_PERMISSION_PROTOCOL_VERSION.to_string(),
                token: "wrong".to_string(),
                request: InternalToolPermissionRequest {
                    tool_id: "browser".to_string(),
                    input: json!({"action":"snapshot"}),
                },
            },
            Some(&mut handler),
        );

        assert!(!response.is_allowed());
        assert_eq!(
            response.reason.as_deref(),
            Some("invalid internal permission token")
        );
    }
}
