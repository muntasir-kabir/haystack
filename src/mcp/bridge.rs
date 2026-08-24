//! Private IPC client for the temporary MCP server owned by the GUI.

use std::io::{BufRead, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

use super::{ok, protocol_response, session, tool_err, ServerState};

/// Authenticated client for the GUI's private local IPC socket. Public MCP
/// clients communicate with Logotomy only over stdio; this socket and its port
/// are internal routing details.
#[derive(Clone)]
pub(crate) struct GuiClient {
    session: session::GuiSession,
}

impl GuiClient {
    pub(crate) fn from_current_session() -> Result<Self, String> {
        Ok(Self {
            session: session::load()?,
        })
    }

    pub(crate) fn attach(session_id: &str) -> Result<Self, String> {
        let client = Self {
            session: session::load_for_id(session_id)?,
        };
        client.ping()?;
        Ok(client)
    }

    #[cfg(test)]
    pub(crate) fn from_session(session: session::GuiSession) -> Self {
        Self { session }
    }

    pub(crate) fn request(&self, message: &Value) -> Result<Option<String>, String> {
        forward_to(&self.session, message)
    }

    fn ping(&self) -> Result<(), String> {
        let request = json!({
            "jsonrpc": "2.0",
            "id": "logotomy-attach-check",
            "method": "ping",
            "params": {}
        });
        let response = self
            .request(&request)?
            .ok_or_else(|| "GUI session did not answer the attachment check".to_string())?;
        let response: Value = serde_json::from_str(&response)
            .map_err(|_| "GUI session returned an invalid attachment response".to_string())?;
        if response.get("result").is_none() {
            return Err("GUI session rejected the attachment check".to_string());
        }
        Ok(())
    }
}

pub fn run_gui_bridge() {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    let mut local_state = ServerState::default();
    local_state.enable_gui_mode();

    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            continue;
        };
        let params = message.get("params").cloned().unwrap_or(Value::Null);
        let id = message.get("id").cloned();

        // Discovery and schema operations are served locally so configured
        // clients can start even before the GUI begins sharing a log.
        if matches!(
            method,
            "server/discover"
                | "initialize"
                | "ping"
                | "tools/list"
                | "resources/list"
                | "resources/read"
        ) {
            if let Some(id) = id {
                let response = protocol_response(id, method, &params, &mut local_state);
                let _ = writeln!(output, "{response}");
                let _ = output.flush();
            }
            continue;
        }

        match forward(&message) {
            Ok(Some(response)) => {
                let _ = writeln!(output, "{response}");
                let _ = output.flush();
            }
            Ok(None) => {}
            Err(error) => {
                if let Some(id) = id {
                    let response = ok(id, tool_err(&format!(
                        "Logotomy GUI session unavailable: {error}. Open a log in Logotomy and choose Start MCP, then retry."
                    )));
                    let _ = writeln!(output, "{response}");
                    let _ = output.flush();
                }
            }
        }
    }
}

fn forward(message: &Value) -> Result<Option<String>, String> {
    GuiClient::from_current_session()?.request(message)
}

fn forward_to(session: &session::GuiSession, message: &Value) -> Result<Option<String>, String> {
    let mut stream = TcpStream::connect(("127.0.0.1", session.port))
        .map_err(|e| format!("cannot connect to private GUI socket: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| format!("cannot configure bridge timeout: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| format!("cannot configure bridge timeout: {e}"))?;

    let request = json!({
        "session_id": session.session_id,
        "request": message,
    })
    .to_string();
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("failed to send GUI request: {e}"))?;
    stream
        .shutdown(Shutdown::Write)
        .map_err(|e| format!("failed to finish GUI request: {e}"))?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| format!("failed to read GUI response: {e}"))?;
    if message.get("id").is_none() {
        return Ok(None);
    }
    if response.is_empty() {
        return Err("GUI session rejected the request".to_string());
    }
    serde_json::from_str::<Value>(&response)
        .map_err(|e| format!("GUI returned invalid JSON-RPC: {e}"))?;
    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::{run_gui_ipc, ServerState};
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc, Mutex};

    #[test]
    fn unavailable_bridge_error_is_a_tool_result() {
        let response = ok(json!(1), tool_err("GUI unavailable"));
        let response: Value = serde_json::from_str(&response).unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["result"]["isError"], true);
    }

    #[test]
    fn bridge_forwards_authenticated_requests_to_gui_ipc() {
        let mut gui_state = ServerState::default();
        gui_state.enable_gui_mode();
        let state = Arc::new(Mutex::new(gui_state));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (bound_tx, bound_rx) = mpsc::channel::<Result<u16, String>>();
        let session_id = session::generate_session_id();
        let server_session_id = session_id.clone();
        let stop = Arc::clone(&shutdown);
        let server = std::thread::spawn(move || {
            let _ = run_gui_ipc(0, state, stop, Some(bound_tx), server_session_id);
        });
        let port = bound_rx.recv().unwrap().unwrap();
        let gui_session = session::GuiSession {
            pid: std::process::id(),
            port,
            session_id,
            started_at_epoch_ms: 0,
        };
        let request = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/list",
            "params": {}
        });
        let response = GuiClient::from_session(gui_session)
            .request(&request)
            .unwrap()
            .unwrap();
        let response: Value = serde_json::from_str(&response).unwrap();
        assert!(response["result"]["tools"].is_array());

        shutdown.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }
}
