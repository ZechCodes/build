//! A detached, one-shot helper swaps the bridge after the daemon has stopped.
//! The helper owns a stopped store snapshot and rolls it and the binary back
//! unless a new daemon reports sustained, versioned readiness.

use std::fs;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::HelperResult;

const HEALTH_DEADLINE: Duration = Duration::from_secs(60);
const HEALTH_STABLE: Duration = Duration::from_secs(30);
const HEALTH_FRESH: u64 = 5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    #[serde(default)]
    pub backup_protocol: u8,
    pub nonce: String,
    pub running_pid: u32,
    pub staged_digest: String,
    pub version: String,
    pub installed_binary: PathBuf,
    pub staged_binary: PathBuf,
    pub tasks_dir: PathBuf,
    pub home: PathBuf,
    pub uid: String,
}

#[derive(Serialize, Deserialize)]
struct BackupReady {
    nonce: String,
    protocol: u8,
    binary_digest: String,
    running_digest: String,
    marker_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Health {
    nonce: String,
    version: String,
    pid: u32,
    binary: PathBuf,
    timestamp: u64,
}

#[derive(Debug)]
enum TransactionFailure {
    Recovered(String),
    Pending(String),
}

impl TransactionFailure {
    fn message(&self) -> &str {
        match self {
            Self::Recovered(message) | Self::Pending(message) => message,
        }
    }
    fn pending(&self) -> bool {
        matches!(self, Self::Pending(_))
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub fn updates_dir(home: &Path) -> PathBuf {
    home.join(".build/updates")
}
pub fn result_path(home: &Path) -> PathBuf {
    updates_dir(home).join("result.json")
}
fn active_path(home: &Path) -> PathBuf {
    updates_dir(home).join("active.json")
}
fn job_file(dir: &Path) -> PathBuf {
    dir.join("job.json")
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    let temporary = path.with_extension("tmp");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.set_permissions(fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    fs::rename(&temporary, path).map_err(|e| e.to_string())?;
    sync_parent(path)
}

pub fn create_job(
    home: &Path,
    tasks_dir: &Path,
    installed_binary: &Path,
    version: &str,
    attempt_id: &str,
) -> Result<(PathBuf, Job), String> {
    uuid::Uuid::parse_str(attempt_id).map_err(|_| "invalid update attempt id")?;
    let nonce = attempt_id.to_string();
    let root = updates_dir(home);
    let tasks = fs::canonicalize(tasks_dir).map_err(|e| format!("invalid task store: {e}"))?;
    if fs::symlink_metadata(tasks_dir)
        .map_err(|e| e.to_string())?
        .file_type()
        .is_symlink()
    {
        return Err("symlinked task store cannot be updated safely".into());
    }
    let root_parent = fs::canonicalize(root.parent().ok_or("updates directory has no parent")?)
        .map_err(|e| e.to_string())?;
    let root = root_parent.join("updates");
    if tasks.starts_with(&root) || root.starts_with(&tasks) {
        return Err("task store overlaps update workspace".into());
    }
    fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    sync_parent(&root)?;
    let jobs = root.join("jobs");
    fs::create_dir_all(&jobs).map_err(|e| e.to_string())?;
    sync_parent(&jobs)?;
    let dir = root.join("jobs").join(&nonce);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    sync_parent(&dir)?;
    let uid = Command::new("id")
        .arg("-u")
        .output()
        .map_err(|e| e.to_string())?;
    if !uid.status.success() {
        return Err("could not determine user id".into());
    }
    let job = Job {
        backup_protocol: 1,
        nonce,
        running_pid: std::process::id(),
        staged_digest: String::new(),
        version: version.to_string(),
        installed_binary: installed_binary.to_path_buf(),
        staged_binary: dir.join("staged-build-bridge"),
        tasks_dir: tasks,
        home: home.to_path_buf(),
        uid: String::from_utf8_lossy(&uid.stdout).trim().to_string(),
    };
    write_json(&job_file(&dir), &job)?;
    Ok((dir, job))
}

pub fn record_staged(dir: &Path) -> Result<(), String> {
    let mut job = load_job(dir)?;
    sync_file(&job.staged_binary)?;
    sync_parent(&job.staged_binary)?;
    job.staged_digest = super::provenance::binary_digest(&job.staged_binary)?;
    write_json(&job_file(dir), &job)
}

pub fn active_attempt(home: &Path) -> Result<Option<String>, String> {
    let path = active_path(home);
    match fs::read(&path) {
        Ok(bytes) => {
            let job: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            let dir = job
                .staged_binary
                .parent()
                .ok_or("update job has no directory")?;
            // The manager can report failure after launching the independent
            // helper. An unheld flock or old marker cannot prove completion.
            let _ = helper_running(dir)?;
            Ok(Some(job.nonce))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn helper_running(dir: &Path) -> Result<bool, String> {
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("helper.lock"))
        .map_err(|e| e.to_string())?;
    let result = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        unsafe {
            libc::flock(lock.as_raw_fd(), libc::LOCK_UN);
        }
        Ok(false)
    } else if std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock {
        Ok(true)
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}

pub fn probation_active(home: &Path) -> bool {
    if !active_path(home).exists() {
        return false;
    }
    // A crashed helper is restarted by its own service-manager job. Until it
    // reports a completed recovery, the candidate store stays quarantined.
    true
}

/// A daemon calls this only after it can serve requests. Calls during an update
/// write a nonce/version/PID-stamped beat; ordinary daemon starts do nothing.
pub fn heartbeat(home: &Path, running_version: &str) -> Result<(), String> {
    let active = active_path(home);
    let bytes = match fs::read(&active) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let job: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if job.version != running_version {
        return Ok(());
    }
    let binary = fs::canonicalize(std::env::current_exe().map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let installed = fs::canonicalize(&job.installed_binary).map_err(|e| e.to_string())?;
    if binary != installed {
        return Ok(());
    }
    let health = Health {
        nonce: job.nonce,
        version: running_version.into(),
        pid: std::process::id(),
        binary,
        timestamp: now(),
    };
    let dir = job
        .staged_binary
        .parent()
        .ok_or("update job has no directory")?;
    write_json(&dir.join("health.json"), &health)
}

/// Start a separate service-manager job from a copy of this binary. It is not
/// part of the bridge unit, so stopping that unit cannot stop the helper.
pub fn launch(dir: &Path) -> Result<(), String> {
    let mut job = load_job(dir)?;
    verify_staged_digest(&job)?;
    super::provenance::managed_binary(&job.home, &job.installed_binary)?;
    job.running_pid = std::process::id();
    write_json(&job_file(dir), &job)?;
    let helper = dir.join("update-helper");
    fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, &helper)
        .map_err(|e| e.to_string())?;
    fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    sync_file(&helper)?;
    sync_parent(&helper)?;
    // The helper and every directory entry leading to it must survive a
    // restart before active.json promises that recovery can launch it.
    sync_ancestors_through(dir, &job.home)?;
    write_json(&active_path(&job.home), &job)?;
    let started = match std::env::consts::OS {
        "linux" => launch_systemd(&helper, dir, &job),
        "macos" => launch_launchd(&helper, dir, &job),
        _ => Err("unsupported bridge service manager".into()),
    };
    // Even a failed or timed-out launcher may have started the detached
    // helper. Keep ownership and probation until its terminal result clears
    // the active marker.
    started
}

fn launch_systemd(helper: &Path, dir: &Path, _job: &Job) -> Result<(), String> {
    let unit = format!("build-bridge-update-{}", uuid::Uuid::new_v4());
    let status = run_command(
        Command::new("systemd-run")
            .args([
                "--user",
                "--collect",
                "--property=Restart=on-failure",
                "--property=RestartSec=2s",
                "--unit",
                &unit,
                "--",
            ])
            .arg(helper)
            .arg("update-helper")
            .arg(dir),
        Duration::from_secs(30),
    )?;
    if status.success() {
        Ok(())
    } else {
        Err("systemd-run could not launch updater".into())
    }
}

fn launch_launchd(helper: &Path, dir: &Path, job: &Job) -> Result<(), String> {
    // A helper plist in LaunchAgents would run again at every future login.
    // Bootstrap this private one-shot plist directly from the job directory.
    let launch_id = uuid::Uuid::new_v4();
    let path = dir.join(format!("update-helper-{launch_id}.plist"));
    let xml = format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>ing.getbuild.bridge.update.{launch_id}</string><key>ProgramArguments</key><array><string>{}</string><string>update-helper</string><string>{}</string></array><key>RunAtLoad</key><true/><key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict></dict></plist>", xml_escape(helper), xml_escape(dir));
    fs::write(&path, xml).map_err(|e| e.to_string())?;
    sync_file(&path)?;
    sync_parent(&path)?;
    let status = run_command(
        Command::new("launchctl")
            .arg("bootstrap")
            .arg(format!("gui/{}", job.uid))
            .arg(&path),
        Duration::from_secs(30),
    )?;
    if status.success() {
        Ok(())
    } else {
        Err("launchctl could not launch updater".into())
    }
}

fn xml_escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn load_job(dir: &Path) -> Result<Job, String> {
    let bytes = fs::read(job_file(dir)).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|e| e.to_string())
}

pub fn run_helper(dir: &Path) -> Result<(), String> {
    let job = load_job(dir)?;
    if dir.join("completed").exists() {
        return remove_active_if_matching(&job);
    }
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(dir.join("helper.lock"))
        .map_err(|e| e.to_string())?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err("update helper is already running".into());
    }
    if dir.join("completed").exists() {
        return remove_active_if_matching(&job);
    }
    if let Ok(bytes) = fs::read(result_path(&job.home)) {
        if let Ok(result) = serde_json::from_slice::<HelperResult>(&bytes) {
            if result.attempt_id == job.nonce && !result.rollback_pending {
                complete_job(&job, dir, &result)?;
                return remove_active_if_matching(&job);
            }
        }
    }
    let started = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dir.join("started"));
    let fresh = match started {
        Ok(file) => {
            file.sync_all().map_err(|e| e.to_string())?;
            sync_parent(&dir.join("started"))?;
            true
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => false,
        Err(error) => return Err(error.to_string()),
    };
    let outcome = if fresh {
        install(&job, dir)
    } else {
        recover(&job, dir)
    };
    let rollback_pending = outcome
        .as_ref()
        .err()
        .is_some_and(TransactionFailure::pending);
    let result = HelperResult {
        success: outcome.is_ok(),
        version: job.version.clone(),
        attempt_id: job.nonce.clone(),
        rollback_pending,
        error: outcome.as_ref().err().map(|e| e.message().to_string()),
    };
    write_json(&result_path(&job.home), &result)?;
    if rollback_pending {
        return Err(outcome.unwrap_err().message().to_string());
    }
    complete_job(&job, dir, &result)?;
    remove_active_if_matching(&job)
}

fn complete_job(job: &Job, dir: &Path, result: &HelperResult) -> Result<(), String> {
    if job.nonce != result.attempt_id {
        return Err("terminal attempt mismatch".into());
    }
    write_json(&dir.join("terminal.json"), result)?;
    let path = dir.join("completed");
    let file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .map_err(|e| e.to_string())?;
    if let Err(error) = file
        .sync_all()
        .map_err(|e| e.to_string())
        .and_then(|()| sync_parent(&path))
    {
        let _ = fs::remove_file(&path);
        return Err(error);
    }
    Ok(())
}

fn remove_active_if_matching(job: &Job) -> Result<(), String> {
    let path = active_path(&job.home);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let active: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if active.nonce != job.nonce {
        return Ok(());
    }
    fs::remove_file(&path).map_err(|e| e.to_string())?;
    sync_parent(&path)
}

/// Re-launch an interrupted helper after a machine restart. The helper's
/// started marker makes this a rollback, never a second install attempt.
pub fn ensure_recovery(home: &Path) -> Result<(), String> {
    let bytes = match fs::read(active_path(home)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.to_string()),
    };
    let job: Job = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    let dir = job
        .staged_binary
        .parent()
        .ok_or("update job has no directory")?;
    if helper_running(dir)? {
        return Ok(());
    }
    let age = fs::metadata(active_path(home))
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|modified| SystemTime::now().duration_since(modified).ok());
    if age.is_some_and(|age| age < Duration::from_secs(10)) {
        return Ok(());
    }
    let helper = dir.join("update-helper");
    // Set the retry grace before launch: a fast recovery may remove active
    // before the launcher returns, and writing afterwards would resurrect it.
    write_json(&active_path(home), &job)?;
    match std::env::consts::OS {
        "linux" => launch_systemd(&helper, dir, &job),
        "macos" => launch_launchd(&helper, dir, &job),
        _ => Err("unsupported bridge service manager".into()),
    }
}

fn recover(job: &Job, dir: &Path) -> Result<(), TransactionFailure> {
    let binary = dir.join("previous-build-bridge");
    let running = dir.join("running-build-bridge");
    let marker = dir.join("previous-install-marker");
    let ready = dir.join("backup-ready.json");
    match fs::symlink_metadata(&ready) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return Err(TransactionFailure::Pending(
                "invalid backup checkpoint type".into(),
            ))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if job.backup_protocol != 1 {
                return Err(TransactionFailure::Pending(
                    "legacy update has no verified backup checkpoint; manual recovery required"
                        .into(),
                ));
            }
            return Err(TransactionFailure::Recovered(
                "update helper interrupted before backup checkpoint; previous bridge was untouched"
                    .into(),
            ));
        }
        Err(error) => return Err(TransactionFailure::Pending(error.to_string())),
    }
    let checkpoint: BackupReady = fs::read(&ready)
        .map_err(|e| TransactionFailure::Pending(e.to_string()))
        .and_then(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|e| TransactionFailure::Pending(format!("invalid backup checkpoint: {e}")))
        })?;
    validate_backups(job, &binary, &running, &marker, &checkpoint)
        .map_err(|e| TransactionFailure::Pending(format!("invalid rollback backup: {e}")))?;
    let store = job
        .tasks_dir
        .with_extension(format!("bridge-update-{}", job.nonce));
    rollback(job, &binary, &store, &marker)
        .map_err(|e| TransactionFailure::Pending(format!("rollback failed: {e}")))?;
    Err(TransactionFailure::Recovered(
        "update helper restarted after interruption; restored previous bridge".into(),
    ))
}

fn install(job: &Job, dir: &Path) -> Result<(), TransactionFailure> {
    validate_manager_pid(job).map_err(TransactionFailure::Recovered)?;
    let live_path = if std::env::consts::OS == "linux" {
        PathBuf::from(format!("/proc/{}/exe", job.running_pid))
    } else {
        job.installed_binary.clone()
    };
    install_with(
        job,
        dir,
        &live_path,
        &mut |action| service_command(job, action),
        &mut await_health,
    )
}

fn install_with(
    job: &Job,
    dir: &Path,
    live_path: &Path,
    service: &mut dyn FnMut(&str) -> Result<(), String>,
    health: &mut dyn FnMut(&Job, &Path) -> Result<(), String>,
) -> Result<(), TransactionFailure> {
    let verified_binary =
        verified_staged_snapshot(job, dir).map_err(TransactionFailure::Recovered)?;
    let old_binary = dir.join("previous-build-bridge");
    let running_binary = dir.join("running-build-bridge");
    let old_store = job
        .tasks_dir
        .with_extension(format!("bridge-update-{}", job.nonce));
    let old_marker = dir.join("previous-install-marker");
    publish_backup(&job.installed_binary, &old_binary).map_err(TransactionFailure::Recovered)?;
    publish_backup(live_path, &running_binary).map_err(|e| {
        TransactionFailure::Recovered(format!("could not save running bridge: {e}"))
    })?;
    if super::provenance::binary_digest(&running_binary).map_err(TransactionFailure::Recovered)?
        != super::provenance::binary_digest(&old_binary).map_err(TransactionFailure::Recovered)?
    {
        return Err(TransactionFailure::Recovered(
            "running bridge differs from the managed install".into(),
        ));
    }
    publish_backup(&super::provenance::marker_path(&job.home), &old_marker)
        .map_err(TransactionFailure::Recovered)?;
    let checkpoint = backup_checkpoint(job, &old_binary, &running_binary, &old_marker)
        .map_err(TransactionFailure::Recovered)?;
    validate_backups(job, &old_binary, &running_binary, &old_marker, &checkpoint)
        .map_err(TransactionFailure::Recovered)?;
    verify_staged_digest(job).map_err(TransactionFailure::Recovered)?;
    write_json(&dir.join("backup-ready.json"), &checkpoint)
        .map_err(TransactionFailure::Recovered)?;
    if let Err(error) = service("stop") {
        return match service("start") {
            Ok(()) => Err(TransactionFailure::Recovered(format!(
                "could not stop bridge: {error}"
            ))),
            Err(start) => Err(TransactionFailure::Pending(format!(
                "could not stop bridge: {error}; restart failed: {start}"
            ))),
        };
    }
    let operation = (|| {
        if job.tasks_dir.exists() {
            fs::rename(&job.tasks_dir, &old_store).map_err(|e| e.to_string())?;
            sync_parent(&old_store)?;
            sync_tree(&old_store)?;
            copy_tree(&old_store, &job.tasks_dir)?;
            sync_parent(&job.tasks_dir)?;
        }
        verify_staged_digest(job)?;
        atomic_replace_verified(&verified_binary, &job.installed_binary, &job.staged_digest)?;
        super::provenance::write_marker(&job.home, &job.installed_binary)?;
        service("start")?;
        health(job, dir)
    })();
    match operation {
        Ok(()) => Ok(()),
        Err(error) => {
            match validate_backups(job, &old_binary, &running_binary, &old_marker, &checkpoint)
                .and_then(|()| rollback_with(job, &old_binary, &old_store, &old_marker, service))
            {
                Ok(()) => Err(TransactionFailure::Recovered(error)),
                Err(rollback) => Err(TransactionFailure::Pending(format!(
                    "{error}; rollback failed: {rollback}"
                ))),
            }
        }
    }
}

fn publish_backup(source: &Path, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension("tmp");
    fs::copy(source, &temporary).map_err(|e| e.to_string())?;
    sync_file(&temporary)?;
    fs::rename(&temporary, destination).map_err(|e| e.to_string())?;
    sync_parent(destination)
}

fn backup_checkpoint(
    job: &Job,
    binary: &Path,
    running: &Path,
    marker: &Path,
) -> Result<BackupReady, String> {
    Ok(BackupReady {
        nonce: job.nonce.clone(),
        protocol: job.backup_protocol,
        binary_digest: super::provenance::binary_digest(binary)?,
        running_digest: super::provenance::binary_digest(running)?,
        marker_digest: super::provenance::binary_digest(marker)?,
    })
}

fn validate_backups(
    job: &Job,
    binary: &Path,
    running: &Path,
    marker: &Path,
    ready: &BackupReady,
) -> Result<(), String> {
    let actual = backup_checkpoint(job, binary, running, marker)?;
    if actual.nonce != ready.nonce
        || actual.protocol != ready.protocol
        || actual.protocol != 1
        || actual.binary_digest != ready.binary_digest
        || actual.running_digest != ready.running_digest
        || actual.marker_digest != ready.marker_digest
        || actual.running_digest != actual.binary_digest
    {
        return Err("rollback backup changed after checkpoint".into());
    }
    super::provenance::validate_saved_marker(marker, binary, &job.installed_binary)
}

fn rollback(job: &Job, binary: &Path, store: &Path, marker: &Path) -> Result<(), String> {
    rollback_with(job, binary, store, marker, &mut |action| {
        service_command(job, action)
    })
}

fn rollback_with(
    job: &Job,
    binary: &Path,
    store: &Path,
    marker: &Path,
    service: &mut dyn FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    service("stop")?;
    atomic_replace(binary, &job.installed_binary)?;
    super::provenance::restore_marker(&job.home, marker)?;
    if store.exists() {
        let rejected = job
            .tasks_dir
            .with_extension(format!("rejected-update-{}", job.nonce));
        if job.tasks_dir.exists() {
            fs::rename(&job.tasks_dir, &rejected).map_err(|e| e.to_string())?;
        }
        fs::rename(store, &job.tasks_dir).map_err(|e| e.to_string())?;
        sync_parent(&job.tasks_dir)?;
    }
    service("start")
}

fn verify_staged_version(binary: &Path, version: &str) -> Result<(), String> {
    let mut command = Command::new(binary);
    command.arg("--version");
    let output = bounded_version_output(&mut command, Duration::from_secs(10))
        .map_err(|error| format!("staged bridge version probe failed: {error}"))?;
    if !output.status.success() || output.stdout != format!("build-bridge {version}\n").as_bytes() {
        return Err("staged bridge reports the wrong version".into());
    }
    Ok(())
}

fn bounded_version_output(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    // Linux can briefly reject execution while another thread's fork still
    // holds the snapshot's writable descriptor. Only this transient errno is
    // retried, and the probe still runs before the service is stopped.
    let mut retries = 0;
    let mut child = loop {
        match command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => break child,
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && retries < 4 => {
                retries += 1;
                thread::sleep(Duration::from_millis(20));
            }
            Err(error) => return Err(error.to_string()),
        }
    };
    wait_child(&mut child, timeout)?;
    child.wait_with_output().map_err(|e| e.to_string())
}

fn verify_staged_digest(job: &Job) -> Result<(), String> {
    if job.staged_digest.len() != 64
        || super::provenance::binary_digest(&job.staged_binary)? != job.staged_digest
    {
        return Err("staged bridge digest changed after verification".into());
    }
    Ok(())
}

fn verified_staged_snapshot(job: &Job, dir: &Path) -> Result<PathBuf, String> {
    // Check before executing any candidate code. Run the version probe from
    // a private copy so a changing staged file cannot be installed later.
    verify_staged_digest(job)?;
    let snapshot = dir.join("verified-build-bridge");
    fs::copy(&job.staged_binary, &snapshot).map_err(|e| e.to_string())?;
    fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    sync_file(&snapshot)?;
    sync_parent(&snapshot)?;
    verify_binary_digest(&snapshot, &job.staged_digest)?;
    verify_staged_digest(job)?;
    verify_staged_version(&snapshot, &job.version)?;
    // A script or native executable can modify its own inode while probing.
    verify_binary_digest(&snapshot, &job.staged_digest)?;
    verify_staged_digest(job)?;
    Ok(snapshot)
}

fn verify_binary_digest(binary: &Path, expected: &str) -> Result<(), String> {
    if super::provenance::binary_digest(binary)? != expected {
        return Err("staged bridge digest changed after verification".into());
    }
    Ok(())
}

fn atomic_replace_verified(source: &Path, destination: &Path, digest: &str) -> Result<(), String> {
    verify_binary_digest(source, digest)?;
    let temporary = destination.with_extension(format!("update-{}", uuid::Uuid::new_v4()));
    fs::copy(source, &temporary).map_err(|e| e.to_string())?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    verify_binary_digest(&temporary, digest)?;
    sync_file(&temporary)?;
    sync_parent(&temporary)?;
    fs::rename(&temporary, destination).map_err(|e| e.to_string())?;
    sync_parent(destination)
}

fn atomic_replace(source: &Path, destination: &Path) -> Result<(), String> {
    let temporary = destination.with_extension(format!("update-{}", uuid::Uuid::new_v4()));
    fs::copy(source, &temporary).map_err(|e| e.to_string())?;
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())?;
    fs::File::open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())?;
    fs::rename(&temporary, destination).map_err(|e| e.to_string())?;
    sync_parent(destination)
}

fn sync_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("update path has no parent")?;
    fs::File::open(parent)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())
}

fn sync_ancestors_through(dir: &Path, home: &Path) -> Result<(), String> {
    if dir == home {
        return Ok(());
    }
    let mut ancestor = dir.parent();
    while let Some(path) = ancestor {
        sync_file(path)?;
        if path == home {
            return Ok(());
        }
        ancestor = path.parent();
    }
    Err("update job is outside home".into())
}

fn sync_file(path: &Path) -> Result<(), String> {
    fs::File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|e| e.to_string())
}

fn service_command(job: &Job, action: &str) -> Result<(), String> {
    let stopping_pid = if action == "stop" {
        Some(manager_current_pid(job)?)
    } else {
        None
    };
    let mut command = match std::env::consts::OS {
        "linux" => {
            let mut c = Command::new("systemctl");
            c.args(["--user", action, "build-bridge.service"]);
            c
        }
        "macos" => {
            let mut c = Command::new("launchctl");
            let target = format!("gui/{}/ing.getbuild.bridge", job.uid);
            match action {
                "stop" => {
                    c.arg("bootout").arg(target);
                }
                _ => {
                    c.arg("bootstrap").arg(format!("gui/{}", job.uid)).arg(
                        job.home
                            .join("Library/LaunchAgents/ing.getbuild.bridge.plist"),
                    );
                }
            }
            c
        }
        _ => return Err("unsupported service manager".into()),
    };
    let status = run_command(&mut command, Duration::from_secs(30))?;
    let absent_launchd = stop_failure_is_absent(action, std::env::consts::OS, stopping_pid);
    if !status.success() && !absent_launchd {
        return Err(format!("bridge service {action} failed"));
    }
    if action == "stop" {
        if let Some(Some(pid)) = stopping_pid {
            wait_pid_exit(pid, Duration::from_secs(15))?;
        }
    }
    Ok(())
}

fn stop_failure_is_absent(action: &str, os: &str, observed: Option<Option<u32>>) -> bool {
    action == "stop" && os == "macos" && observed == Some(None)
}

fn validate_manager_pid(job: &Job) -> Result<(), String> {
    if manager_current_pid(job)? == Some(job.running_pid) {
        Ok(())
    } else {
        Err("running bridge is not the managed service process".into())
    }
}

fn manager_current_pid(job: &Job) -> Result<Option<u32>, String> {
    let mut command = match std::env::consts::OS {
        "linux" => {
            let mut c = Command::new("systemctl");
            c.args([
                "--user",
                "show",
                "-p",
                "MainPID",
                "--value",
                "build-bridge.service",
            ]);
            c
        }
        "macos" => {
            let mut c = Command::new("launchctl");
            c.arg("print")
                .arg(format!("gui/{}/ing.getbuild.bridge", job.uid));
            c
        }
        _ => return Err("unsupported service manager".into()),
    };
    let output = bounded_output(&mut command, Duration::from_secs(10))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if std::env::consts::OS == "macos" && stderr.contains("could not find service") {
            return Ok(None);
        }
        return Err("cannot inspect bridge service process".into());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(parse_manager_pid(&text, std::env::consts::OS).filter(|pid| *pid != 0))
}

fn parse_manager_pid(text: &str, os: &str) -> Option<u32> {
    if os == "linux" {
        text.trim().parse::<u32>().ok()
    } else {
        text.lines().find_map(|line| {
            line.trim()
                .strip_prefix("pid = ")
                .and_then(|v| v.parse::<u32>().ok())
        })
    }
}

fn process_alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as i32, 0) != 0 } {
        return false;
    }
    if std::env::consts::OS == "linux" {
        if let Ok(stat) = fs::read_to_string(format!("/proc/{pid}/stat")) {
            if stat
                .rsplit_once(") ")
                .is_some_and(|(_, rest)| rest.starts_with('Z'))
            {
                return false;
            }
        }
    }
    true
}

fn wait_pid_exit(pid: u32, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    while process_alive(pid) {
        if Instant::now() >= deadline {
            return Err("bridge process did not stop".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

fn run_command(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    wait_child(&mut child, timeout)
}

fn bounded_output(
    command: &mut Command,
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    wait_child(&mut child, timeout)?;
    child.wait_with_output().map_err(|e| e.to_string())
}

fn wait_child(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err("update command timed out".into());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn await_health(job: &Job, dir: &Path) -> Result<(), String> {
    let deadline = Instant::now() + HEALTH_DEADLINE;
    let mut stable: Option<(u32, Instant)> = None;
    while Instant::now() < deadline {
        let fresh = fs::read(dir.join("health.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Health>(&bytes).ok())
            .filter(|beat| valid_health(job, beat));
        match fresh {
            Some(beat) => match stable {
                Some((pid, start)) if pid == beat.pid && start.elapsed() >= HEALTH_STABLE => {
                    return Ok(())
                }
                Some((pid, _)) if pid == beat.pid => {}
                _ => stable = Some((beat.pid, Instant::now())),
            },
            None => stable = None,
        }
        thread::sleep(Duration::from_secs(1));
    }
    Err("new bridge did not remain healthy for 30 seconds within 60 seconds".into())
}

fn valid_health(job: &Job, beat: &Health) -> bool {
    beat.nonce == job.nonce
        && beat.version == job.version
        && beat.timestamp <= now()
        && now().saturating_sub(beat.timestamp) <= HEALTH_FRESH
        && process_alive(beat.pid)
        && beat.binary == fs::canonicalize(&job.installed_binary).unwrap_or_default()
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir(target).map_err(|e| e.to_string())?;
    for entry in fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let dest = target.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &dest)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &dest).map_err(|e| e.to_string())?;
            fs::set_permissions(
                &dest,
                fs::metadata(entry.path())
                    .map_err(|e| e.to_string())?
                    .permissions(),
            )
            .map_err(|e| e.to_string())?;
            fs::File::open(&dest)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
        } else {
            return Err("task store contains unsupported file type".into());
        }
    }
    fs::set_permissions(
        target,
        fs::metadata(source)
            .map_err(|e| e.to_string())?
            .permissions(),
    )
    .map_err(|e| e.to_string())?;
    fs::File::open(target)
        .and_then(|dir| dir.sync_all())
        .map_err(|e| e.to_string())
}

fn sync_tree(dir: &Path) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_dir() {
            sync_tree(&entry.path())?;
        } else if ty.is_file() {
            sync_file(&entry.path())?;
        } else {
            return Err("task store contains unsupported file type".into());
        }
    }
    sync_file(dir)
}

#[cfg(test)]
mod tests;
