use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::core::settings::Settings;

const SESSION_FILE: &str = "mcp-gui-session.json";
const SESSION_ID_BYTES: usize = 6;
const SESSION_ID_HEX_LEN: usize = SESSION_ID_BYTES * 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GuiSession {
    pub pid: u32,
    pub port: u16,
    pub session_id: String,
    pub started_at_epoch_ms: u128,
}

pub fn generate_session_id() -> String {
    let mut bytes = [0u8; SESSION_ID_BYTES];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut session_id = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(session_id, "{byte:02x}");
    }
    session_id
}

pub fn session_path() -> PathBuf {
    Settings::home_dir().join(SESSION_FILE)
}

pub fn publish(port: u16, session_id: String) -> Result<GuiSession, String> {
    let session = GuiSession {
        pid: std::process::id(),
        port,
        session_id,
        started_at_epoch_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    };
    publish_at(&session_path(), &session)?;
    Ok(session)
}

fn publish_at(path: &Path, session: &GuiSession) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("invalid MCP session path {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let bytes = serde_json::to_vec(session)
        .map_err(|e| format!("failed to encode MCP GUI session: {e}"))?;
    write_private(&temporary, &bytes)
        .map_err(|e| format!("failed to write {}: {e}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(format!("failed to publish {}: {error}", path.display()));
    }
    Ok(())
}

fn write_private(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

pub fn load() -> Result<GuiSession, String> {
    load_at(&session_path())
}

/// Load the current GUI session only when the user-supplied capability ID
/// matches it. Keep every authentication failure deliberately generic so an
/// invalid caller cannot distinguish a missing, expired, or different session.
pub fn load_for_id(session_id: &str) -> Result<GuiSession, String> {
    load_for_id_at(&session_path(), session_id)
}

fn load_for_id_at(path: &Path, session_id: &str) -> Result<GuiSession, String> {
    if session_id.len() != SESSION_ID_HEX_LEN
        || !session_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("GUI session ID is invalid or expired".to_string());
    }
    let session = load_at(path).map_err(|_| "GUI session ID is invalid or expired".to_string())?;
    if !constant_time_eq(session.session_id.as_bytes(), session_id.as_bytes()) {
        return Err("GUI session ID is invalid or expired".to_string());
    }
    Ok(session)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (&left, &right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

pub(crate) fn session_id_matches(expected: &str, supplied: &str) -> bool {
    supplied.len() == SESSION_ID_HEX_LEN
        && supplied.bytes().all(|byte| byte.is_ascii_hexdigit())
        && constant_time_eq(expected.as_bytes(), supplied.as_bytes())
}

fn load_at(path: &Path) -> Result<GuiSession, String> {
    let bytes = std::fs::read(path).map_err(|e| {
        format!(
            "no active Logotomy GUI MCP session at {}: {e}",
            path.display()
        )
    })?;
    let session: GuiSession = serde_json::from_slice(&bytes)
        .map_err(|e| format!("invalid MCP GUI session manifest: {e}"))?;
    if session.port == 0
        || session.session_id.len() != SESSION_ID_HEX_LEN
        || !session
            .session_id
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("invalid MCP GUI session manifest values".to_string());
    }
    Ok(session)
}

pub fn clear() {
    let _ = std::fs::remove_file(session_path());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_session_ids_are_48_bit_hex_and_unique() {
        let first = generate_session_id();
        let second = generate_session_id();
        assert_eq!(first.len(), SESSION_ID_HEX_LEN);
        assert!(first.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn manifest_round_trips_atomically() {
        let path = std::env::temp_dir().join(format!(
            "logotomy-mcp-session-{}-{}.json",
            std::process::id(),
            rand::random::<u64>()
        ));
        let session = GuiSession {
            pid: 42,
            port: 4321,
            session_id: generate_session_id(),
            started_at_epoch_ms: 123,
        };
        publish_at(&path, &session).unwrap();
        assert_eq!(load_at(&path).unwrap(), session);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn invalid_manifest_values_are_rejected() {
        let path = std::env::temp_dir().join(format!(
            "logotomy-mcp-session-invalid-{}-{}.json",
            std::process::id(),
            rand::random::<u64>()
        ));
        let invalid = br#"{"pid":1,"port":0,"session_id":"tiny","started_at_epoch_ms":0}"#;
        write_private(&path, invalid).unwrap();
        assert!(load_at(&path).unwrap_err().contains("invalid"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn constant_time_comparison_checks_every_byte() {
        assert!(constant_time_eq(b"same", b"same"));
        assert!(!constant_time_eq(b"same", b"lame"));
        assert!(!constant_time_eq(b"short", b"longer"));
    }

    #[test]
    fn session_id_must_match_current_private_manifest() {
        let path = std::env::temp_dir().join(format!(
            "logotomy-mcp-session-auth-{}-{}.json",
            std::process::id(),
            rand::random::<u64>()
        ));
        let session_id = generate_session_id();
        let session = GuiSession {
            pid: 42,
            port: 4321,
            session_id: session_id.clone(),
            started_at_epoch_ms: 123,
        };
        publish_at(&path, &session).unwrap();
        assert_eq!(load_for_id_at(&path, &session_id).unwrap(), session);
        for invalid in ["tiny".to_string(), generate_session_id()] {
            assert_eq!(
                load_for_id_at(&path, &invalid).unwrap_err(),
                "GUI session ID is invalid or expired"
            );
        }
        assert!(!session_id_matches(
            &session_id,
            &(session_id.clone() + "0")
        ));
        let _ = std::fs::remove_file(path);
    }
}
