//! Frozen config-loader semantics from 4e1eb6b47a79dffb015d62a7d4d26c9b3a5ab8f3.
//!
//! This is a self-contained copy of the old loader's config-visible operations:
//! path selection, canonical-path deduplication, id reservation, source decoding,
//! counter restoration and isolation overwrite. Orchestrator construction and
//! observation publication are omitted; they do not affect loaded config. Do
//! not replace this with calls to the current loader.
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct LegacyLoader {
    pub(super) projects: Vec<Value>,
    pub(super) parked: Vec<Value>,
    next_project: u64,
}

impl LegacyLoader {
    pub(super) fn load(config: &Value) -> Self {
        let mut loader = Self {
            next_project: config.get("next_project").and_then(Value::as_u64).unwrap_or(1).max(1),
            ..Self::default()
        };
        for project in config.get("projects").and_then(Value::as_array).into_iter().flatten() {
            loader.restore(project);
        }
        loader
    }

    fn restore(&mut self, project: &Value) {
        let Some(path) = primary_path(project) else { return };
        let repo = PathBuf::from(path);
        let number = project.get("id").and_then(Value::as_str)
            .and_then(|id| id.strip_prefix("proj-"))
            .and_then(|id| id.parse::<u64>().ok())
            .filter(|number| *number > 0 && *number < u64::MAX);
        if !repo.exists() {
            if let Some(number) = number { self.next_project = self.next_project.max(number + 1); }
            self.parked.push(project.clone());
            return;
        }
        let repo = std::fs::canonicalize(&repo).unwrap_or(repo);
        let Some(index) = self.register(&repo, project, number) else { return };
        if let Some(entries) = project.get("sources").and_then(Value::as_array) {
            let sources = entries.iter().enumerate().filter_map(|(index, value)| source(index, value)).collect::<Vec<_>>();
            if !sources.is_empty() {
                let next = next_source(&sources);
                let row = &mut self.projects[index];
                row["next_source"] = json!(row["next_source"].as_u64().unwrap().max(next));
                row["sources"] = json!(sources);
            }
        }
        let row = &mut self.projects[index];
        if let Some(next) = project.get("next_source").and_then(Value::as_u64) {
            row["next_source"] = json!(row["next_source"].as_u64().unwrap().max(next));
        }
        // The old loader sets isolation even when an entry has no override.
        row["isolation"] = project.get("isolation").filter(|isolation| matches!(isolation.as_str(), Some("rift" | "worktree"))).cloned().unwrap_or(Value::Null);
    }

    fn register(&mut self, repo: &Path, project: &Value, number: Option<u64>) -> Option<usize> {
        if let Some(index) = self.projects.iter().position(|row| row["path"] == json!(repo)) { return Some(index); }
        let number = number.unwrap_or(self.next_project);
        let id = format!("proj-{number}");
        if self.projects.iter().any(|row| row["id"] == id) { return None; }
        let name = repo.file_name().and_then(|name| name.to_str()).unwrap_or("project");
        let base = project.get("base_branch").and_then(Value::as_str).unwrap_or("main");
        let initial_source = json!({"id":"source-1", "name":name, "mount":mount_name(name), "path":repo,
            "is_git":repo.join(".git").exists(), "base_branch":base, "sync_base":null});
        self.projects.push(json!({"id":id,"name":name,"path":repo,"base_branch":base,
            "sources":[initial_source], "next_source":2,"isolation":null}));
        self.next_project = self.next_project.max(number + 1);
        Some(self.projects.len() - 1)
    }
}

fn primary_path(project: &Value) -> Option<&str> {
    project.get("path").and_then(Value::as_str).or_else(|| {
        project.get("sources")?.as_array()?.first()?.get("path")?.as_str()
    })
}

fn source(index: usize, value: &Value) -> Option<Value> {
    let named = value.get("path")?.as_str()?;
    let path = if let Some(rest) = named.strip_prefix("~/") {
        PathBuf::from(std::env::var("HOME").ok()?).join(rest)
    } else { PathBuf::from(named) };
    let path = std::fs::canonicalize(path).ok()?;
    if !path.is_dir() { return None; }
    let fallback = path.file_name()?.to_string_lossy();
    let name = value.get("name").and_then(Value::as_str).unwrap_or(&fallback);
    Some(json!({
        "id":value.get("id").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("source-{}", index + 1)),
        "name":name,"mount":mount_name(value.get("mount").and_then(Value::as_str).unwrap_or(name)),
        "path":path,"is_git":path.join(".git").exists(),
        "base_branch":value.get("base_branch").and_then(Value::as_str).unwrap_or("main"),
        "sync_base":value.get("sync_base").and_then(Value::as_bool)
    }))
}

fn next_source(sources: &[Value]) -> u64 {
    sources.iter().filter_map(|source| source["id"].as_str()?.strip_prefix("source-")?.parse::<u64>().ok()).max().unwrap_or(0) + 1
}

fn mount_name(name: &str) -> String {
    let mount = name.trim().chars().map(|character| {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') { character } else { '-' }
    }).collect::<String>();
    let mount = mount.trim_matches(['.', '-']);
    if mount.is_empty() || mount == ".." { "source".to_string() } else { mount.to_string() }
}
