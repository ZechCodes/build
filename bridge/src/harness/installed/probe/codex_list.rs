//! Codex's own model list, asked of `codex app-server`.
//!
//! The list depends on the CLI and on the account (the server refreshes it),
//! so only the CLI can say it. `model/list` is the app-server protocol's own
//! verb; `codex debug models` would say the same but is a debugging command.
//! The requests are written together and stdin is held open until both are
//! answered: an app-server that reads end-of-input exits without answering.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Instant;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{version_in, CliProbe, PROBE_DEADLINE};
use crate::harness::installed::{CliReading, ListedModel};

/// Pages followed before the list is taken as it stands. One page holds every
/// model today; the bound is against a server that never stops paging.
const MAX_PAGES: u64 = 10;

const INITIALIZE_ID: u64 = 1;

pub struct CodexModelList;

pub static CODEX_MODEL_LIST: CodexModelList = CodexModelList;

impl CliProbe for CodexModelList {
    fn read(&self, binary: &str) -> CliReading {
        match AppServer::start(binary).and_then(|server| server.read()) {
            Ok(reading) => reading,
            Err(why) => {
                eprintln!("cli probe: {binary} app-server model/list: {why}");
                CliReading::default()
            }
        }
    }
}

/// One short-lived `codex app-server`, ended whatever it answered.
struct AppServer {
    child: Child,
    lines: Receiver<Value>,
    expiry: Instant,
}

impl AppServer {
    fn start(binary: &str) -> std::io::Result<Self> {
        let mut command = Command::new(binary);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let dir = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let mut child = command
            .arg("app-server")
            .env_remove("CLAUDECODE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .current_dir(dir)
            .spawn()?;
        let lines = read_lines(child.stdout.take());
        Ok(Self {
            child,
            lines,
            expiry: Instant::now() + PROBE_DEADLINE,
        })
    }

    fn read(mut self) -> std::io::Result<CliReading> {
        self.send(&json!({
            "id": INITIALIZE_ID,
            "method": "initialize",
            "params": {
                "clientInfo": { "name": "build_bridge_probe", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": {},
            },
        }))?;
        self.send(&json!({ "method": "initialized" }))?;
        let initialized = self.answer(INITIALIZE_ID)?;
        let version = initialized["userAgent"].as_str().and_then(version_in);
        let listed = self.list().map_err(|why| eprintln!("cli probe: codex model/list: {why}")).ok();
        Ok(CliReading { version, listed })
    }

    /// Every page of `model/list`, hidden models included.
    fn list(&mut self) -> std::io::Result<Vec<ListedModel>> {
        let mut listed = Vec::new();
        let mut cursor: Option<String> = None;
        for page in 0..MAX_PAGES {
            let id = INITIALIZE_ID + 1 + page;
            self.send(&json!({
                "id": id,
                "method": "model/list",
                "params": { "includeHidden": true, "cursor": cursor },
            }))?;
            let answered: ModelListPage = serde_json::from_value(self.answer(id)?)
                .map_err(|why| std::io::Error::other(format!("unreadable model/list: {why}")))?;
            listed.extend(answered.data.into_iter().map(ListedModel::from));
            match answered.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(listed),
            }
        }
        Ok(listed)
    }

    fn send(&mut self, message: &Value) -> std::io::Result<()> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .ok_or_else(|| std::io::Error::other("app-server stdin is closed"))?;
        writeln!(stdin, "{message}")?;
        stdin.flush()
    }

    /// The result answering request `id`, passing over notifications and
    /// anything else said first.
    fn answer(&mut self, id: u64) -> std::io::Result<Value> {
        loop {
            let left = self.expiry.saturating_duration_since(Instant::now());
            let line = self.lines.recv_timeout(left).map_err(|unread| match unread {
                RecvTimeoutError::Timeout => std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("no answer within {}s", PROBE_DEADLINE.as_secs()),
                ),
                RecvTimeoutError::Disconnected => {
                    std::io::Error::other("app-server closed its output")
                }
            })?;
            if line["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = line.get("error") {
                return Err(std::io::Error::other(format!("app-server refused: {error}")));
            }
            return Ok(line["result"].clone());
        }
    }
}

impl Drop for AppServer {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Every JSON line the server writes, on a thread that ends with its output.
fn read_lines(stdout: Option<ChildStdout>) -> Receiver<Value> {
    let (lines, received) = std::sync::mpsc::channel();
    if let Some(stdout) = stdout {
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                let Ok(value) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                if lines.send(value).is_err() {
                    return;
                }
            }
        });
    }
    received
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelListPage {
    #[serde(default)]
    data: Vec<ListedEntry>,
    #[serde(default)]
    next_cursor: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedEntry {
    id: String,
    /// The model a thread is started with; the preset `id` beside it has
    /// matched it on every entry seen, and is the fallback.
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    supported_reasoning_efforts: Vec<ListedEffort>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListedEffort {
    reasoning_effort: String,
}

impl From<ListedEntry> for ListedModel {
    fn from(entry: ListedEntry) -> Self {
        let id = entry.model.unwrap_or(entry.id);
        ListedModel {
            label: entry.display_name.unwrap_or_else(|| id.clone()),
            id,
            hidden: entry.hidden,
            efforts: entry
                .supported_reasoning_efforts
                .into_iter()
                .map(|effort| effort.reasoning_effort)
                .collect(),
        }
    }
}
