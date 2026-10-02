//! A bridge installed by the published installer has an explicit local marker.
//! Development binaries and a service pointed at another executable are read only,
//! except that a development binary the service runs can be replaced by a
//! verified release when the person confirms it (`replaceable_development_binary`).

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

/// A development or unmarked binary the platform service runs, which a
/// confirmed install may replace in place with a verified release. The marker
/// is not consulted: it makes no claim about this binary. A bridge started by
/// hand, or a service starting another executable, is never replaceable: the
/// helper restarts the service, which would not run what it wrote.
pub fn replaceable_development_binary(
    home: &Path,
    running_binary: &Path,
) -> Result<PathBuf, String> {
    let running = fs::canonicalize(running_binary)
        .map_err(|error| format!("running bridge path is invalid: {error}"))?;
    let unit = service_unit(home)?;
    let text = fs::read_to_string(&unit).map_err(|_| {
        "this bridge does not run as the bridge service; install a release build".to_string()
    })?;
    if !unit_runs_binary(&text, &running) {
        return Err(
            "the bridge service runs another executable; install a release build".to_string(),
        );
    }
    Ok(running)
}

/// The check an install is made under: the managed install's marker, or, for
/// a confirmed replacement, a development binary the service runs.
pub fn installable_binary(
    home: &Path,
    binary: &Path,
    replaces_development_build: bool,
) -> Result<PathBuf, String> {
    if replaces_development_build {
        replaceable_development_binary(home, binary)
    } else {
        managed_binary(home, binary)
    }
}

/// Whether `binary` sits in a cargo target directory, which cargo tags with
/// a `CACHEDIR.TAG` it wrote. The next `cargo build` there overwrites
/// whatever replaced the binary.
pub fn in_cargo_target_dir(binary: &Path) -> bool {
    binary
        .ancestors()
        .skip(1)
        .any(|dir| is_cargo_cache_tag(&dir.join("CACHEDIR.TAG")))
}

/// Reads only a regular file, and only the prefix cargo's tag fits in, so a
/// FIFO or a huge file by that name cannot stall or bloat startup.
fn is_cargo_cache_tag(path: &Path) -> bool {
    use std::io::Read;
    const TAG_PREFIX: u64 = 512;
    if !fs::metadata(path).is_ok_and(|metadata| metadata.is_file()) {
        return false;
    }
    let mut tag = String::new();
    fs::File::open(path)
        .and_then(|file| file.take(TAG_PREFIX).read_to_string(&mut tag))
        .is_ok()
        && tag.starts_with("Signature: 8a477f597d28d172789f06886806bc55")
        && tag.contains("created by cargo")
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
/// before the old service can be restarted after rollback. An empty saved
/// marker stands for none: a replaced development build had no marker.
pub fn restore_marker(home: &Path, saved: &Path) -> Result<(), String> {
    let content = fs::read(saved).map_err(|e| e.to_string())?;
    if content.is_empty() {
        return remove_marker(home);
    }
    write_marker_content(home, &content)
}

fn remove_marker(home: &Path) -> Result<(), String> {
    let path = marker_path(home);
    match fs::remove_file(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    }
    fs::File::open(path.parent().ok_or("bridge marker has no parent")?)
        .and_then(|parent| parent.sync_all())
        .map_err(|e| e.to_string())
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

    /// `in_cargo_target_dir` on another thread, failing rather than hanging
    /// the suite if it blocks.
    fn probe_within_seconds(binary: &Path) -> bool {
        let binary = binary.to_path_buf();
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || sender.send(in_cargo_target_dir(&binary)));
        receiver
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("the cargo target probe returns without blocking")
    }

    #[test]
    fn a_cargo_tag_that_is_not_a_small_regular_file_is_not_read_whole() {
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let fifo_dir = dir.path().join("fifo");
        let binary = fifo_dir.join("release/build-bridge");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();
        let fifo = std::ffi::CString::new(fifo_dir.join("CACHEDIR.TAG").as_os_str().as_bytes())
            .unwrap();
        // SAFETY: a valid NUL-terminated path; mkfifo only creates the node.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(!probe_within_seconds(&binary));

        // Only a short prefix is read: the cargo line past it is never seen.
        let padded_dir = dir.path().join("padded");
        let padded = padded_dir.join("release/build-bridge");
        fs::create_dir_all(padded.parent().unwrap()).unwrap();
        fs::write(&padded, b"binary").unwrap();
        let mut tag = b"Signature: 8a477f597d28d172789f06886806bc55\n".to_vec();
        tag.extend(std::iter::repeat_n(b'#', 4096));
        tag.extend(b"\n# created by cargo\n");
        fs::write(padded_dir.join("CACHEDIR.TAG"), tag).unwrap();
        assert!(!probe_within_seconds(&padded));
    }

    #[test]
    fn a_binary_under_a_cargo_target_directory_is_one_cargo_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("custom-target");
        let binary = target.join("release/build-bridge");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"binary").unwrap();
        assert!(!in_cargo_target_dir(&binary));
        fs::write(
            target.join("CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n\
             # This file is a cache directory tag created by cargo.\n",
        )
        .unwrap();
        assert!(in_cargo_target_dir(&binary));

        let other = dir.path().join("elsewhere/build-bridge");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, b"binary").unwrap();
        fs::write(
            dir.path().join("elsewhere/CACHEDIR.TAG"),
            "Signature: 8a477f597d28d172789f06886806bc55\n# created by another tool\n",
        )
        .unwrap();
        assert!(!in_cargo_target_dir(&other));
    }

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

    fn write_service_for(home: &Path, binary: &Path) {
        let unit = service_unit(home).unwrap();
        fs::create_dir_all(unit.parent().unwrap()).unwrap();
        let config = ServiceConfig {
            binary_path: binary.to_path_buf(),
            log_dir: home.join(".build/log"),
            env: vec![],
        };
        let manager = crate::service::manager_for(std::env::consts::OS).unwrap();
        fs::write(&unit, manager.render_unit(&config)).unwrap();
    }

    #[test]
    fn development_binary_the_service_runs_is_replaceable_with_or_without_a_marker() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let binary = home.join("src/target/release/build-bridge");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        fs::write(&binary, b"source build").unwrap();
        write_service_for(home, &binary);
        assert!(managed_binary(home, &binary).is_err());
        assert_eq!(
            replaceable_development_binary(home, &binary).unwrap(),
            fs::canonicalize(&binary).unwrap()
        );
        // A stale marker (a source build copied over a release) changes nothing.
        fs::create_dir_all(home.join(".build")).unwrap();
        fs::write(marker_path(home), "/elsewhere/build-bridge\nabc\n").unwrap();
        assert!(replaceable_development_binary(home, &binary).is_ok());
    }

    #[test]
    fn development_binary_is_not_replaceable_unless_the_service_runs_it() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let binary = home.join("build-bridge");
        fs::write(&binary, b"source build").unwrap();
        // No service at all: the bridge was started by hand.
        assert!(replaceable_development_binary(home, &binary).is_err());
        // A service that starts another executable would never run the release.
        let other = home.join("other/build-bridge");
        fs::create_dir_all(other.parent().unwrap()).unwrap();
        fs::write(&other, b"release").unwrap();
        write_service_for(home, &other);
        assert!(replaceable_development_binary(home, &binary).is_err());
    }

    #[test]
    fn an_empty_saved_marker_restores_as_no_marker() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".build")).unwrap();
        fs::write(marker_path(home), "/a/build-bridge\nabc\n").unwrap();
        let saved = home.join("saved-marker");
        fs::write(&saved, b"").unwrap();
        restore_marker(home, &saved).unwrap();
        assert!(!marker_path(home).exists());
        // Restoring absence twice is still absence.
        restore_marker(home, &saved).unwrap();
        assert!(!marker_path(home).exists());
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
