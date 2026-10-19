//! Observer acknowledgements, separate from the extension's append-only log.
use super::{PiTelemetryConfig, State, CONFIRMED, UNAVAILABLE};
use serde_json::json;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Version(u64, u64, u8, bool);

pub(super) fn publish(
    config: &PiTelemetryConfig,
    state: &State,
    confirmation: u8,
    last: &mut Option<Version>,
) -> std::io::Result<()> {
    let version = Version(
        state.generation,
        state.sequence,
        confirmation,
        state.shutdown,
    );
    if state.generation == 0 || *last == Some(version) {
        return Ok(());
    }
    // A broken acknowledgement sink must not cause repeated I/O at poll rate.
    // Failure changes confirmation once, allowing one withdrawal attempt.
    *last = Some(version);
    let status = match confirmation {
        UNAVAILABLE => "unsupported",
        CONFIRMED if !state.shutdown => "ready",
        _ => "starting",
    };
    let identity = &config.policy.launch_identity;
    let value = json!({
        "owner": config.owner, "thread_id": identity.thread_id,
        "launch_id": identity.id, "generation": state.generation,
        "sequence": state.sequence, "status": status,
    });
    let mut path = config.path.as_os_str().to_os_string();
    path.push(".ack");
    atomic_private_write(
        &std::path::PathBuf::from(path),
        value.to_string().as_bytes(),
    )
}

fn atomic_private_write(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("ack-tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        file.write_all(bytes)?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
