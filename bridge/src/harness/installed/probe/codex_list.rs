//! Codex's own model list, asked of `codex app-server`.
//!
//! The list depends on the CLI and on the account (the server refreshes it),
//! so only the CLI can say it. `model/list` is the app-server protocol's own
//! verb; `codex debug models` would say the same but is a debugging command.
//! The requests are written together and stdin is held open until both are
//! answered: an app-server that reads end-of-input exits without answering.

use std::io::Write;

use serde::Deserialize;
use serde_json::{json, Value};

use super::{version_in, CliProbe, ProbeChild};
use crate::harness::installed::{CliReading, ListedModel};

/// Pages followed before the list is taken as it stands. One page holds every
/// model today; the bound is against a server that never stops paging.
const MAX_PAGES: u64 = 10;

const INITIALIZE_ID: u64 = 1;

/// The most models kept from one list. Codex lists nine today.
const MAX_MODELS: usize = 200;

/// The longest label kept, in characters.
const MAX_LABEL: usize = 80;

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
    child: ProbeChild,
}

impl AppServer {
    fn start(binary: &str) -> std::io::Result<Self> {
        Ok(Self {
            child: ProbeChild::start(binary, &["app-server"], true)?,
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
        let listed = self
            .list()
            .map_err(|why| eprintln!("cli probe: codex model/list: {why}"))
            .ok();
        Ok(CliReading { version, listed })
    }

    /// Every page of `model/list`, hidden models included, less any entry
    /// that could not be a model id.
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
            listed.extend(
                answered
                    .data
                    .into_iter()
                    .filter_map(ListedModel::from_listed),
            );
            listed.truncate(MAX_MODELS);
            match answered.next_cursor {
                Some(next) => cursor = Some(next),
                None => return Ok(listed),
            }
        }
        Ok(listed)
    }

    fn send(&mut self, message: &Value) -> std::io::Result<()> {
        let stdin = self.child.stdin()?;
        writeln!(stdin, "{message}")?;
        stdin.flush()
    }

    /// The result answering request `id`, passing over notifications and
    /// anything else said first.
    fn answer(&mut self, id: u64) -> std::io::Result<Value> {
        loop {
            let line = self
                .child
                .next_line()?
                .ok_or_else(|| std::io::Error::other("app-server closed its output"))?;
            let Ok(said) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if said["id"].as_u64() != Some(id) {
                continue;
            }
            if let Some(error) = said.get("error") {
                return Err(std::io::Error::other(format!(
                    "app-server refused: {}",
                    clipped(&error.to_string(), MAX_LABEL)
                )));
            }
            return Ok(said["result"].clone());
        }
    }
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

impl ListedModel {
    /// What Build keeps of one entry: its id only when it is one a session
    /// could be started on (the same shape [`crate::models::ModelChoice`]
    /// accepts), its label clipped, its efforts as said — they are matched
    /// against the harness's own before any is offered.
    fn from_listed(entry: ListedEntry) -> Option<Self> {
        let id = entry.model.unwrap_or(entry.id);
        if !crate::models::is_model_id(&id) {
            return None;
        }
        Some(ListedModel {
            label: clipped(entry.display_name.as_deref().unwrap_or(&id), MAX_LABEL),
            id,
            hidden: entry.hidden,
            efforts: entry
                .supported_reasoning_efforts
                .into_iter()
                .map(|effort| clipped(&effort.reasoning_effort, MAX_LABEL))
                .collect(),
        })
    }
}

fn clipped(text: &str, most: usize) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(most)
        .collect()
}
