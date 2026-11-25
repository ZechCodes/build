//! Private connector inputs and the pinned child lifecycle (Linux only).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::os::unix::{
    fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    process::CommandExt,
};
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Deserialize;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::watch;

use super::claim::Assignment;
use crate::http::tls::{self, LoadedTls};

const CONFIG: &str = "connector.yml";
const CREDENTIALS: &str = "connector.json";
const CA: &str = "origin-ca.pem";
const MAX_LOG_LINE: usize = 256 * 1024;
const READY_MESSAGE: &str = "Registered tunnel connection";
const CONFIG_MESSAGE: &str = "Updated to new configuration";

pub(super) struct PrivateRuntime {
    path: PathBuf,
    lock: File,
}

impl PrivateRuntime {
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err("cannot create connector runtime directory".into()),
        }
        let metadata = fs::symlink_metadata(&path)
            .map_err(|_| "cannot inspect connector runtime directory")?;
        if !metadata.is_dir()
            || !owned(&metadata, 0o700)
            || path.canonicalize().ok().as_deref() != Some(&path)
        {
            return Err("connector runtime directory must be private and unaliased".into());
        }
        let lock = open_lock(&path.join("lock"))?;
        let runtime = Self { path, lock };
        runtime.clean_inputs()?;
        Ok(runtime)
    }

    pub fn prepare(
        &self,
        assignment: &Assignment,
        address: SocketAddr,
        ca_pem: &str,
    ) -> Result<(), String> {
        self.clean_inputs()?;
        let credential = zeroize::Zeroizing::new(
            serde_json::to_vec(&assignment.credentials)
                .map_err(|_| "cannot encode connector credential")?,
        );
        self.write(CREDENTIALS, &credential)?;
        self.write(CA, ca_pem.as_bytes())?;
        let config = ingress(&assignment.hostname, address, &self.path.join(CA));
        self.write(
            CONFIG,
            &serde_json::to_vec(&config).map_err(|_| "cannot encode connector ingress")?,
        )
    }

    pub fn command(&self, binary: &Path, assignment: &Assignment) -> Command {
        self.with_inputs(tunnel_command(binary), assignment)
    }

    fn with_inputs(&self, mut command: Command, assignment: &Assignment) -> Command {
        command
            .arg("--config")
            .arg(self.path.join(CONFIG))
            .args(["run", "--credentials-file"])
            .arg(self.path.join(CREDENTIALS))
            .arg(&assignment.credentials.tunnel_id)
            .current_dir(&self.path);
        command
    }

    pub fn quarantined(&self, generation: u64) -> Result<bool, String> {
        let path = self.path.join(format!("rejected-{generation}"));
        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.is_file() && owned(&metadata, 0o600) => Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
            _ => Err("unsafe connector quarantine marker".into()),
        }
    }

    pub fn quarantine(&self, generation: u64) -> Result<(), String> {
        if self.quarantined(generation)? {
            return Ok(());
        }
        let name = format!("rejected-{generation}");
        let mut file = self.create(&name)?;
        file.write_all(b"edge configuration or output refused\n")
            .and_then(|()| file.sync_all())
            .and_then(|()| File::open(&self.path)?.sync_all())
            .map_err(|_| "cannot persist connector quarantine marker".into())
    }

    pub fn clean_inputs(&self) -> Result<(), String> {
        let [credentials, config, ca] =
            [CREDENTIALS, CONFIG, CA].map(|name| remove_owned(&self.path.join(name)));
        credentials.and(config).and(ca)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> Result<(), String> {
        self.create(name)?
            .write_all(bytes)
            .map_err(|_| "cannot write private connector file".into())
    }

    fn create(&self, name: &str) -> Result<File, String> {
        OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.path.join(name))
            .map_err(|_| "cannot create private connector file".into())
    }
}

fn tunnel_command(binary: &Path) -> Command {
    let mut command = Command::new(binary);
    command.args([
        "tunnel",
        "--no-autoupdate",
        "--no-prechecks",
        "--metrics",
        "127.0.0.1:0",
        "--output",
        "json",
        "--loglevel",
        "info",
        "--management-diagnostics=false",
    ]);
    command
}

impl Drop for PrivateRuntime {
    fn drop(&mut self) {
        let _ = self.clean_inputs();
        let _ = rustix::fs::flock(&self.lock, rustix::fs::FlockOperation::Unlock);
    }
}

fn open_lock(path: &Path) -> Result<File, String> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| "cannot open connector runtime lock")?;
    let metadata = lock
        .metadata()
        .map_err(|_| "cannot inspect connector runtime lock")?;
    if !metadata.is_file() || !owned(&metadata, 0o600) || metadata.nlink() != 1 {
        return Err("unsafe connector runtime lock".into());
    }
    rustix::fs::flock(&lock, rustix::fs::FlockOperation::NonBlockingLockExclusive)
        .map_err(|_| "connector runtime is already in use")?;
    Ok(lock)
}

fn owned(metadata: &fs::Metadata, mode: u32) -> bool {
    metadata.uid() == rustix::process::geteuid().as_raw() && metadata.mode() & 0o7777 == mode
}

fn remove_owned(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && owned(&metadata, 0o600) && metadata.nlink() == 1 => {
            fs::remove_file(path).map_err(|_| "cannot retire private connector file".into())
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        _ => Err("unsafe connector runtime file".into()),
    }
}

fn ingress(host: &str, address: SocketAddr, ca: &Path) -> serde_json::Value {
    serde_json::json!({"ingress": [
        {"hostname": host, "service": format!("https://{address}"), "originRequest": {
            "originServerName": host, "caPool": ca
        }},
        {"service": "http_status:404"}
    ]})
}

pub(super) fn origin_tls(host: &str) -> Result<(LoadedTls, String), String> {
    use rcgen::{
        BasicConstraints, CertificateParams, ExtendedKeyUsagePurpose, IsCa, KeyPair,
        KeyUsagePurpose,
    };
    let mut ca = CertificateParams::default();
    ca.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    ca.key_usages = vec![KeyUsagePurpose::KeyCertSign];
    let ca_key = KeyPair::generate().map_err(|_| "cannot generate connector origin CA key")?;
    let ca = ca
        .self_signed(&ca_key)
        .map_err(|_| "cannot sign connector origin CA")?;
    let leaf_key = KeyPair::generate().map_err(|_| "cannot generate connector origin key")?;
    let mut leaf =
        CertificateParams::new(vec![host.to_owned()]).map_err(|_| "invalid origin TLS hostname")?;
    leaf.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf = leaf
        .signed_by(&leaf_key, &ca, &ca_key)
        .map_err(|_| "cannot sign connector origin certificate")?;
    let private_key = zeroize::Zeroizing::new(leaf_key.serialize_pem());
    let origin = tls::from_pem_bytes(leaf.pem().as_bytes(), private_key.as_bytes())?;
    Ok((origin, ca.pem()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum ChildStatus {
    Starting,
    Connected,
    Exited,
    ConfigChanged,
    OutputFailed,
}

pub(super) struct RunningConnector {
    pub status: watch::Receiver<ChildStatus>,
    stop: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl RunningConnector {
    pub fn start(mut command: Command) -> Result<Self, String> {
        configure_child(&mut command);
        let child = command.spawn().map_err(|_| "connector failed to start")?;
        let (status, receiver) = watch::channel(ChildStatus::Starting);
        let (stop, stopped) = watch::channel(false);
        let task = tokio::spawn(watch_child(child, status, stopped));
        Ok(Self {
            status: receiver,
            stop,
            task: Some(task),
        })
    }

    pub async fn stop(mut self) -> ChildStatus {
        self.stop.send_replace(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        *self.status.borrow()
    }
}

impl Drop for RunningConnector {
    fn drop(&mut self) {
        // The monitor owns the child and kills/reaps it even if its caller is cancelled.
        self.stop.send_replace(true);
    }
}

fn configure_child(command: &mut Command) {
    command
        .env_clear()
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: this hook invokes only async-signal-safe primitives and uses only
    // copied data. The parent check closes the fork-to-prctl race.
    unsafe {
        command.as_std_mut().pre_exec(move || {
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 || libc::getppid() != parent
            {
                return Err(io::Error::from_raw_os_error(libc::ECHILD));
            }
            Ok(())
        });
    }
}

async fn watch_child(
    mut child: Child,
    status: watch::Sender<ChildStatus>,
    mut stop: watch::Receiver<bool>,
) {
    let mut stderr = BufReader::new(child.stderr.take().expect("connector stderr is piped"));
    let terminal = tokio::select! {
        biased;
        terminal = monitor_output(&mut stderr, &status) => terminal,
        _ = stop.changed() => ChildStatus::Exited,
        _ = child.wait() => ChildStatus::Exited,
    };
    let _ = child.kill().await;
    let _ = child.wait().await;
    status.send_replace(terminal);
}

#[derive(Deserialize)]
struct ConnectorLog<'a> {
    message: &'a str,
}

async fn monitor_output(
    stream: &mut (impl AsyncBufRead + Unpin),
    status: &watch::Sender<ChildStatus>,
) -> ChildStatus {
    let mut line = Vec::new();
    loop {
        match read_log_line(stream, &mut line).await {
            Ok(true) => {}
            Ok(false) => return ChildStatus::Exited,
            Err(_) => return ChildStatus::OutputFailed,
        }
        // Never log or persist child output, which can include remote configuration.
        if let Ok(log) = serde_json::from_slice::<ConnectorLog<'_>>(&line) {
            match log.message {
                CONFIG_MESSAGE => return ChildStatus::ConfigChanged,
                READY_MESSAGE => {
                    status.send_replace(ChildStatus::Connected);
                }
                _ => {}
            }
        }
        line.clear();
    }
}

async fn read_log_line(
    stream: &mut (impl AsyncBufRead + Unpin),
    line: &mut Vec<u8>,
) -> io::Result<bool> {
    loop {
        let available = stream.fill_buf().await?;
        if available.is_empty() {
            return Ok(!line.is_empty());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |position| position + 1);
        if line.len() + take > MAX_LOG_LINE {
            return Err(io::ErrorKind::InvalidData.into());
        }
        line.extend_from_slice(&available[..take]);
        stream.consume(take);
        if newline.is_some() {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests;
