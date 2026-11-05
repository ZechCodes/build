//! Read Codex's effective shell policy without giving the probe launch authority.
use super::{child, ProbeChild, PROBE_DEADLINE};
use crate::pty::HarnessSpec;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::Path;

#[derive(Clone, Copy)]
pub(crate) enum PolicyFailure {
    Config,
    Timeout,
    Unsupported,
    LegacyProfile,
}

impl PolicyFailure {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::Config => "config_error",
            Self::Timeout => "config_timeout",
            Self::Unsupported => "unsupported",
            Self::LegacyProfile => "legacy_profile",
        }
    }
}

pub(crate) fn read(spec: &HarnessSpec, cwd: &Path) -> Result<Value, PolicyFailure> {
    let binary = crate::pty::resolve_binary(spec).map_err(|_| PolicyFailure::Unsupported)?;
    let binary = binary.to_str().ok_or(PolicyFailure::Unsupported)?;
    let trust = crate::harness::codex::trust_override(cwd);
    let mut command = child::command(binary, &["--config", &trust, "app-server", "--stdio"], true);
    command.current_dir(cwd);
    for key in ["CODEX_HOME", "PATH"] {
        if let Some((_, value)) = spec.env.iter().rev().find(|(name, _)| name == key) {
            command.env(key, value);
        }
    }
    let mut probe = ProbeChild::spawn(command, PROBE_DEADLINE).map_err(io_failure)?;
    let result = exchange(&mut probe, cwd);
    if matches!(result, Err(PolicyFailure::Config)) {
        // Inspect bounded diagnostics only to select a fixed reason. Never
        // forward a config response or provider diagnostic to an agent log.
        let diagnostic = probe
            .failure(io::Error::other("policy unavailable"))
            .to_string();
        if diagnostic.to_ascii_lowercase().contains("profile") {
            return Err(PolicyFailure::LegacyProfile);
        }
    }
    result
}

fn exchange(probe: &mut ProbeChild, cwd: &Path) -> Result<Value, PolicyFailure> {
    send(
        probe,
        &json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "build_shell_policy", "version": env!("CARGO_PKG_VERSION")}
        }}),
    )?;
    answer(probe, 1)?;
    send(probe, &json!({"method": "initialized"}))?;
    send(
        probe,
        &json!({"id": 2, "method": "config/read", "params": {"cwd": cwd, "includeLayers": false}}),
    )?;
    let mut result = answer(probe, 2)?;
    let config = result
        .get_mut("config")
        .filter(|value| value.is_object())
        .ok_or(PolicyFailure::Unsupported)?;
    match config
        .get_mut("shell_environment_policy")
        .map(Value::take)
        .unwrap_or(Value::Null)
    {
        policy @ Value::Object(_) => Ok(policy),
        Value::Null => Ok(json!({})),
        _ => Err(PolicyFailure::Config),
    }
}

fn send(probe: &mut ProbeChild, request: &Value) -> Result<(), PolicyFailure> {
    let stdin = probe.stdin().map_err(io_failure)?;
    writeln!(stdin, "{request}").map_err(io_failure)?;
    stdin.flush().map_err(io_failure)
}

fn answer(probe: &mut ProbeChild, id: u64) -> Result<Value, PolicyFailure> {
    while let Some(line) = probe.next_line().map_err(io_failure)? {
        let said: Value = serde_json::from_str(&line).map_err(|_| PolicyFailure::Config)?;
        if said["id"].as_u64() != Some(id) {
            continue;
        }
        if let Some(error) = said.get("error") {
            return Err(native_failure(error));
        }
        return said.get("result").cloned().ok_or(PolicyFailure::Config);
    }
    Err(PolicyFailure::Config)
}

fn native_failure(error: &Value) -> PolicyFailure {
    if error["code"].as_i64() == Some(-32601) {
        return PolicyFailure::Unsupported;
    }
    let message = error["message"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if message.contains("profile") {
        PolicyFailure::LegacyProfile
    } else {
        PolicyFailure::Config
    }
}

fn io_failure(error: io::Error) -> PolicyFailure {
    match error.kind() {
        io::ErrorKind::TimedOut => PolicyFailure::Timeout,
        io::ErrorKind::NotFound | io::ErrorKind::Unsupported => PolicyFailure::Unsupported,
        _ => PolicyFailure::Config,
    }
}
