//! Read Codex's effective shell policy without giving the probe launch authority.
use super::{child, ProbeChild, PROBE_DEADLINE};
use crate::pty::HarnessSpec;
use serde_json::{json, Value};
use std::io::{self, Write};
use std::path::Path;

pub(crate) fn read(spec: &HarnessSpec, cwd: &Path) -> io::Result<Value> {
    let binary = crate::pty::resolve_binary(spec).map_err(|_| unavailable())?;
    let binary = binary.to_str().ok_or_else(unavailable)?;
    let trust = crate::harness::codex::trust_override(cwd);
    let mut command = child::command(binary, &["--config", &trust, "app-server", "--stdio"], true);
    command.current_dir(cwd);
    for key in ["CODEX_HOME", "PATH"] {
        if let Some((_, value)) = spec.env.iter().rev().find(|(name, _)| name == key) {
            command.env(key, value);
        }
    }
    let mut probe = ProbeChild::spawn(command, PROBE_DEADLINE)?;
    send(
        &mut probe,
        &json!({"id": 1, "method": "initialize", "params": {
            "clientInfo": {"name": "build_shell_policy", "version": env!("CARGO_PKG_VERSION")}
        }}),
    )?;
    answer(&mut probe, 1)?;
    send(&mut probe, &json!({"method": "initialized"}))?;
    send(
        &mut probe,
        &json!({"id": 2, "method": "config/read", "params": {"cwd": cwd, "includeLayers": false}}),
    )?;
    let mut result = answer(&mut probe, 2)?;
    let policy = result
        .get_mut("config")
        .and_then(|config| config.get_mut("shell_environment_policy"))
        .map(Value::take)
        .unwrap_or(Value::Null);
    match policy {
        Value::Object(_) => Ok(policy),
        Value::Null => Ok(json!({})),
        _ => Err(unavailable()),
    }
}

fn send(probe: &mut ProbeChild, request: &Value) -> io::Result<()> {
    let stdin = probe.stdin()?;
    writeln!(stdin, "{request}")?;
    stdin.flush()
}

fn answer(probe: &mut ProbeChild, id: u64) -> io::Result<Value> {
    while let Some(line) = probe.next_line()? {
        let said: Value = serde_json::from_str(&line).map_err(|_| unavailable())?;
        if said["id"].as_u64() != Some(id) {
            continue;
        }
        if said.get("error").is_some() {
            return Err(unavailable());
        }
        return said.get("result").cloned().ok_or_else(unavailable);
    }
    Err(unavailable())
}

fn unavailable() -> io::Error {
    io::Error::other("Codex shell policy unavailable")
}
