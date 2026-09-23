//! A bridge installed by the published installer has an explicit local marker.
//! Development binaries and a service pointed at another executable are read only.

use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const MARKER_NAME: &str = "installed-bridge";

pub fn marker_path(home: &Path) -> PathBuf {
    home.join(".build").join(MARKER_NAME)
}

/// Check the marker, the running binary and the platform service unit together.
/// A marker alone must never make an arbitrary development executable updatable.
pub fn managed_binary(home: &Path, running_binary: &Path) -> Result<PathBuf, String> {
    let marker = fs::read_to_string(marker_path(home)).map_err(|_| {
        "this bridge is a development or unmarked install; updates are read only".to_string()
    })?;
    let lines: Vec<&str> = marker.lines().collect();
    if lines.len() != 2 {
        return Err("invalid bridge install marker".into());
    }
    let marked = PathBuf::from(lines[0]);
    let marked = fs::canonicalize(&marked)
        .map_err(|error| format!("installed bridge is missing: {error}"))?;
    let running = fs::canonicalize(running_binary)
        .map_err(|error| format!("running bridge path is invalid: {error}"))?;
    if marked != running || binary_digest(&marked)? != lines[1] {
        return Err("the running bridge does not match the managed install".to_string());
    }
    let unit = service_unit(home)?;
    let text = fs::read_to_string(&unit).map_err(|error| {
        format!(
            "cannot read installed bridge service {}: {error}",
            unit.display()
        )
    })?;
    if !unit_runs_binary(&text, &running) {
        return Err("the bridge service does not point at the managed install".to_string());
    }
    Ok(marked)
}

fn service_unit(home: &Path) -> Result<PathBuf, String> {
    match std::env::consts::OS {
        "linux" => Ok(home.join(".config/systemd/user/build-bridge.service")),
        "macos" => Ok(home.join("Library/LaunchAgents/ing.getbuild.bridge.plist")),
        os => Err(format!("updates are unsupported on {os}")),
    }
}

pub fn binary_digest(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

/// Validate a saved marker against the saved binary before it can be restored.
pub fn validate_saved_marker(saved: &Path, binary: &Path, installed: &Path) -> Result<(), String> {
    let content = fs::read_to_string(saved).map_err(|e| e.to_string())?;
    let mut lines = content.lines();
    let marked = lines.next().ok_or("saved install marker is empty")?;
    let digest = lines.next().ok_or("saved install marker has no digest")?;
    if lines.next().is_some()
        || marked
            != fs::canonicalize(installed)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
        || digest != binary_digest(binary)?
    {
        return Err("saved install marker does not match previous bridge".into());
    }
    Ok(())
}

pub fn write_marker(home: &Path, binary: &Path) -> Result<(), String> {
    let canonical = fs::canonicalize(binary).map_err(|e| e.to_string())?;
    let content = format!("{}\n{}\n", canonical.display(), binary_digest(&canonical)?);
    write_marker_content(home, content.as_bytes())
}

/// Restore exactly the marker saved before the swap. The rename is durable
/// before the old service can be restarted after rollback.
pub fn restore_marker(home: &Path, saved: &Path) -> Result<(), String> {
    let content = fs::read(saved).map_err(|e| e.to_string())?;
    write_marker_content(home, &content)
}

fn write_marker_content(home: &Path, content: &[u8]) -> Result<(), String> {
    let path = marker_path(home);
    let temporary = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(content).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(&temporary, &path).map_err(|e| e.to_string())?;
    fs::File::open(path.parent().ok_or("bridge marker has no parent")?)
        .and_then(|parent| parent.sync_all())
        .map_err(|e| e.to_string())
}

fn unit_runs_binary(text: &str, binary: &Path) -> bool {
    unit_runs_binary_for(text, binary, std::env::consts::OS)
}

fn unit_runs_binary_for(text: &str, binary: &Path, os: &str) -> bool {
    let path = binary.to_string_lossy();
    match os {
        "linux" => {
            let escaped = path
                .replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('%', "%%");
            text.lines()
                .any(|line| line == format!("ExecStart=\"{escaped}\" serve"))
        }
        "macos" => {
            let escaped = path
                .replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;");
            text.contains(&format!("<key>ProgramArguments</key>\n    <array>\n      <string>{escaped}</string>\n      <string>serve</string>"))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::{Launchd, ServiceConfig, ServiceManager, Systemd};

    #[test]
    fn unmarked_development_binary_is_read_only() {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join("build-bridge");
        fs::write(&binary, b"binary").unwrap();
        assert!(managed_binary(dir.path(), &binary).is_err());
    }

    #[test]
    fn marker_must_match_running_binary_and_service() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let binary = home.join("bin/build-bridge");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();
        let unit = service_unit(home).unwrap();
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        let config = ServiceConfig {
            binary_path: binary.clone(),
            log_dir: home.join(".build/log"),
            env: vec![],
        };
        let manager = crate::service::manager_for(std::env::consts::OS).unwrap();
        fs::write(&unit, manager.render_unit(&config)).unwrap();
        fs::create_dir_all(home.join(".build")).unwrap();
        write_marker(home, &binary).unwrap();
        assert_eq!(managed_binary(home, &binary).unwrap(), binary);
        fs::write(&binary, b"source build").unwrap();
        assert!(managed_binary(home, &binary).is_err());
        write_marker(home, &binary).unwrap();
        fs::write(&unit, "ExecStart=/other/build-bridge serve").unwrap();
        assert!(managed_binary(home, &binary).is_err());
    }

    #[test]
    fn native_service_renderers_bind_exact_executable() {
        let binary = Path::new("/tmp/a space/100%<&>/build-bridge");
        let config = ServiceConfig {
            binary_path: binary.to_path_buf(),
            log_dir: PathBuf::from("/tmp/log"),
            env: vec![],
        };
        assert!(unit_runs_binary_for(
            &Systemd.render_unit(&config),
            binary,
            "linux"
        ));
        assert!(unit_runs_binary_for(
            &Launchd.render_unit(&config),
            binary,
            "macos"
        ));
        assert!(!unit_runs_binary_for(
            &Systemd.render_unit(&config),
            Path::new("/tmp/other"),
            "linux"
        ));
        assert!(!unit_runs_binary_for(
            &Launchd.render_unit(&config),
            Path::new("/tmp/other"),
            "macos"
        ));
    }
}
