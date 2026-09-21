use crate::agent_modes::AgentModes;
use crate::app::{AppState, Project};
use crate::isolation::{Isolation, IsolationAvailability, ResolvedIsolation};
use crate::models::{self, AgentProvider, ModelChoice, ProjectAgentChoice};
use serde_json::Value;

mod persistence;
mod settings;

pub(in crate::app) use persistence::isolation_downgrade_note;

/// The harness a bridge nobody has configured creates agents on, and the
/// answer `settings.get` gives until someone chooses otherwise.
pub(in crate::app) const DEFAULT_HARNESS: AgentProvider = AgentProvider::ClaudeAdk;

/// The isolation `entry`'s `"isolation"` key names, or `None` when it names
/// none. A word this bridge does not know is logged under `field` and read as
/// absent — a config written by a newer bridge is not a reason to fail boot,
/// the same answer an unknown `default_harness` gets.
pub(in crate::app) fn configured_isolation(entry: &Value, field: &str) -> Option<Isolation> {
    let named = entry.get("isolation").and_then(Value::as_str)?;
    match Isolation::from_wire(named) {
        Some(isolation) => Some(isolation),
        None => {
            eprintln!("config {field}: unknown {named:?}; using the default");
            None
        }
    }
}

/// The isolation `named` asks for, accepted only when this machine can make
/// it. The wire word is parsed here and nowhere else in the app, and what a
/// volume can lock stays [`IsolationAvailability`]'s fact, so a setter refuses
/// without naming an isolation of its own. Both isolation setters ask it.
pub(in crate::app) fn accept_isolation(
    named: &str,
    available: &IsolationAvailability,
) -> Result<Isolation, String> {
    let asked = Isolation::from_wire(named).ok_or_else(|| {
        format!("unknown isolation {named:?} (expected \"worktree\" or \"rift\")")
    })?;
    match available.lock_reason(asked) {
        None => Ok(asked),
        Some(reason) => Err(format!(
            "Rift isolation is unavailable: {reason}; locked to worktrees"
        )),
    }
}

/// One `settings.set`: every account setting a client named, parsed whole
/// before any of it is applied, so a refusal leaves the account exactly as it
/// was rather than half-moved.
///
/// A setting is one row of [`SettingsPatch::FIELDS`] — its wire key beside the
/// parse that puts the value here — so what this bridge accepts, in what order,
/// and what "nothing to set" means are all the table, never a condition
/// somewhere else that someone must remember to extend.
#[derive(Default)]
pub(in crate::app) struct SettingsPatch {
    pub(in crate::app) projects_dir: Option<std::path::PathBuf>,
    pub(in crate::app) default_harness: Option<AgentProvider>,
    pub(in crate::app) agent_modes: Option<Value>,
    pub(in crate::app) isolation: Option<Isolation>,
    pub(in crate::app) project_agent: Option<ProjectAgentPatch>,
    pub(in crate::app) role_models: Option<crate::models::RoleModels>,
}

/// The list a `settings.set` asks for, refused before anything is written if
/// it names a harness or a model this bridge cannot honour.
///
/// Two refusals of its own. A model declared twice for the same harness is a
/// list where the second entry can never be reached, and the user would have
/// no way to tell which one they were editing. A model with no roles is
/// allowed — it sits in the list unused, which is a legible thing to want.
fn accepted_role_models(value: &Value) -> Result<crate::models::RoleModels, String> {
    let models: crate::models::RoleModels = serde_json::from_value(value.clone())
        .map_err(|why| format!("role_models: {}", plainly(&why.to_string())))?;
    let mut seen: Vec<(Option<AgentProvider>, &str)> = Vec::new();
    for entry in &models.0 {
        if entry.model.trim().is_empty() {
            return Err("Every model in the list needs an id.".to_string());
        }
        let key = (entry.provider, entry.model.as_str());
        if seen.contains(&key) {
            return Err(format!(
                "{} is in the list twice. A model appears once, with all of its roles.",
                entry.model
            ));
        }
        seen.push(key);
        crate::models::ProjectAgentChoice {
            provider: entry.provider,
            model: Some(entry.model.clone()),
            effort: None,
        }
        .resolved(DEFAULT_HARNESS)
        .validate()?;
    }
    Ok(models)
}

/// A serde message a person can read. Serde says "unknown variant `codeing`,
/// expected one of ..." which is most of a sentence already; what it is not is
/// one that starts like the rest of this bridge's refusals.
fn plainly(said: &str) -> String {
    said.split(" at line ").next().unwrap_or(said).to_string()
}

impl SettingsPatch {
    /// Read in this order, so a client that sends both `claude_mode` and
    /// `default_harness` is read by the newer word: they name one setting, and
    /// the later row lands on top of the earlier.
    const FIELDS: [(&'static str, SettingsFieldParse); 8] = [
        ("projects_dir", |patch, value, _| {
            let named = value
                .as_str()
                .ok_or_else(|| "missing required param: projects_dir".to_string())?;
            patch.projects_dir = Some(expand_tilde(named));
            Ok(())
        }),
        ("claude_mode", |patch, value, _| {
            let named = value.as_str().unwrap_or_default();
            patch.default_harness =
                Some(models::carrier_of_claude_mode(named).ok_or_else(|| {
                    format!("unknown claude_mode {named:?} (expected \"headless\" or \"tui\")")
                })?);
            Ok(())
        }),
        ("default_harness", |patch, value, _| {
            let named = value.as_str().unwrap_or_default();
            patch.default_harness = Some(AgentProvider::from_wire(named).ok_or_else(|| {
                format!(
                    "unknown default_harness {named:?} (expected \"claude_adk\", \"claude\", \
                     \"codex\", \"codex_app_server\" or \"pi\")"
                )
            })?);
            Ok(())
        }),
        ("agent_modes", |patch, value, _| {
            patch.agent_modes = Some(value.clone());
            Ok(())
        }),
        ("codex_mode", |patch, value, _| {
            let named = value.as_str().unwrap_or_default();
            patch.default_harness =
                Some(models::carrier_of_codex_mode(named).ok_or_else(|| {
                    format!("unknown codex_mode {named:?} (expected \"headless\" or \"tui\")")
                })?);
            Ok(())
        }),
        ("isolation", |patch, value, available| {
            patch.isolation = Some(accept_isolation(
                value.as_str().unwrap_or_default(),
                available,
            )?);
            Ok(())
        }),
        ("project_agent", |patch, value, _| {
            patch.project_agent = Some(ProjectAgentPatch::parse(value)?);
            Ok(())
        }),
        // The whole grid, not a patch of one cell: it is small, a client that
        // draws it holds all of it, and a merge rule for a two-level map of
        // optional triples is a rule nobody could predict from the wire.
        ("role_models", |patch, value, _| {
            patch.role_models = Some(accepted_role_models(value)?);
            Ok(())
        }),
    ];

    /// The patch `params` asks for, or the refusal a set that names no setting
    /// this bridge knows has earned: a no-op dressed as a mutation says so.
    fn parse(params: &Value, available: &IsolationAvailability) -> Result<Self, String> {
        let mut patch = Self::default();
        let mut named_a_setting = false;
        for (key, parse_field) in Self::FIELDS {
            if let Some(value) = params.get(key) {
                parse_field(&mut patch, value, available)?;
                named_a_setting = true;
            }
        }
        if !named_a_setting {
            return Err("settings.set: nothing to set".to_string());
        }
        Ok(patch)
    }
}

/// One `settings.set`'s words about the project agent.
///
/// Each field is what the client said about that one word: `None` where it said
/// nothing and the device keeps what it has, `Some(value)` where it named one,
/// and `Some(None)` for the `null` that clears it. A `null` object clears all
/// three, which is the same three clearings said at once.
#[derive(Default)]
pub(in crate::app) struct ProjectAgentPatch {
    provider: Option<Option<AgentProvider>>,
    model: Option<Option<String>>,
    effort: Option<Option<String>>,
}

impl ProjectAgentPatch {
    fn parse(value: &Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self {
                provider: Some(None),
                model: Some(None),
                effort: Some(None),
            });
        }
        let object = value
            .as_object()
            .ok_or_else(|| "project_agent must be an object or null".to_string())?;
        Ok(Self {
            provider: object
                .get("provider")
                .map(project_agent_provider)
                .transpose()?,
            model: object
                .get("model")
                .map(|value| project_agent_word(value, "model"))
                .transpose()?,
            effort: object
                .get("effort")
                .map(|value| project_agent_word(value, "effort"))
                .transpose()?,
        })
    }

    /// `current` with this patch laid over it, word by word.
    pub(in crate::app) fn over(&self, current: &ProjectAgentChoice) -> ProjectAgentChoice {
        ProjectAgentChoice {
            provider: self.provider.unwrap_or(current.provider),
            model: self.model.clone().unwrap_or_else(|| current.model.clone()),
            effort: self
                .effort
                .clone()
                .unwrap_or_else(|| current.effort.clone()),
        }
    }
}

/// One word of a project-agent patch. `null` and `""` are both "no preference"
/// — a preference must be able to say it has none — and anything that is not a
/// string is refused rather than read as silence.
fn project_agent_word(value: &Value, field: &str) -> Result<Option<String>, String> {
    match value {
        Value::Null => Ok(None),
        Value::String(text) if text.is_empty() => Ok(None),
        Value::String(text) => Ok(Some(text.clone())),
        _ => Err(format!("project_agent.{field} must be a string or null")),
    }
}

fn project_agent_provider(value: &Value) -> Result<Option<AgentProvider>, String> {
    let Some(named) = project_agent_word(value, "provider")? else {
        return Ok(None);
    };
    AgentProvider::from_wire(&named)
        .map(Some)
        .ok_or_else(|| format!("unknown project_agent provider: {named:?}"))
}

/// What a field does with the value a client sent for it: refuse it, or put it
/// in the patch. Every parse is handed what this volume can make; only
/// isolation has anything to ask it.
type SettingsFieldParse =
    fn(&mut SettingsPatch, &Value, &IsolationAvailability) -> Result<(), String>;

/// Say that sentence on the daemon's log, and hand it back for whoever the
/// create owes it to. Every announcing site goes through here, so the log
/// hears every fallback exactly once and no caller can choose another policy.
pub(crate) fn announce_isolation_downgrade(reason: &str) -> String {
    let note = isolation_downgrade_note(reason);
    eprintln!("{note}");
    note
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("read config {}: {source}", path.display())]
    Read {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse config {}: {source}", path.display())]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum ConfigPersistStep {
    Write,
    Rename,
}

pub(in crate::app) fn read_config(path: &std::path::Path) -> Result<Option<Value>, ConfigError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ConfigError::Read {
                path: path.to_path_buf(),
                source,
            })
        }
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
}

/// Expand a leading `~` / `~/` to the user's home directory; otherwise return the
/// path unchanged. Lets path fields accept `~/code/foo`.
pub(crate) fn expand_tilde(path: &str) -> std::path::PathBuf {
    if path == "~" {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return std::path::Path::new(&home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// The default state root for constructors without an explicit
/// [`HarnessContext`]. A task store must share this parent; attaching one never
/// changes the root.
pub(in crate::app) fn default_state_root() -> std::path::PathBuf {
    expand_tilde("~/.build")
}

impl AppState {
    /// Enable persistence at `path`: load any saved projects + projects-dir from it
    /// (skipping repos that no longer exist), and remember it for future writes.
    pub fn with_config(mut self, path: impl Into<std::path::PathBuf>) -> Result<Self, ConfigError> {
        let path = path.into();
        if let Some(config) = read_config(&path)? {
            self.apply_config(&config);
        }
        self.config_path = Some(path);
        Ok(self)
    }

    fn apply_config(&mut self, config: &Value) {
        if let Some(dir) = config.get("projects_dir").and_then(Value::as_str) {
            self.projects_dir = expand_tilde(dir);
        }
        self.apply_default_harness_config(config);
        self.apply_project_agent_config(config);
        self.apply_role_models_config(config);
        self.apply_agent_modes_config(config);
        if let Some(isolation) = configured_isolation(config, "isolation") {
            self.isolation = isolation;
        }
        self.apply_router_config(config);
        self.restore_configured_projects(config);
    }

    fn apply_default_harness_config(&mut self, config: &Value) {
        let configured = config
            .get("default_harness")
            .and_then(Value::as_str)
            .map(|named| ("default_harness", named, AgentProvider::from_wire(named)))
            .or_else(|| {
                config
                    .get("claude_mode")
                    .and_then(Value::as_str)
                    .map(|named| ("claude_mode", named, models::carrier_of_claude_mode(named)))
            });
        match configured {
            Some((_, _, Some(harness))) => self.default_harness = harness,
            Some((key, named, None)) => {
                eprintln!("config {key}: unknown {named:?}; using the default")
            }
            None => {}
        }
    }

    /// What this device says a project agent starts on, as the config spells
    /// it. A value this bridge cannot read is logged and left absent — a config
    /// a newer bridge wrote is not a reason to fail boot, the same answer an
    /// unknown `default_harness` gets.
    fn apply_project_agent_config(&mut self, config: &Value) {
        let Some(value) = config.get("project_agent") else {
            return;
        };
        match serde_json::from_value::<ProjectAgentChoice>(value.clone()) {
            Ok(choice) => self.project_agent = choice,
            Err(error) => eprintln!("config project_agent: {error}; using the default"),
        }
    }

    /// A grid this bridge cannot read is logged and skipped, the way every
    /// other configured value is: a config file written by a newer bridge must
    /// not stop this one from starting.
    fn apply_role_models_config(&mut self, config: &Value) {
        let Some(value) = config.get("role_models") else {
            return;
        };
        match serde_json::from_value::<crate::models::RoleModels>(value.clone()) {
            Ok(choices) => self.role_models = choices,
            Err(error) => eprintln!("config role_models: {error}; using no grid"),
        }
    }

    fn apply_agent_modes_config(&mut self, config: &Value) {
        self.agent_modes = AgentModes::from_legacy_default(self.default_harness);
        let Some(value) = config.get("agent_modes") else {
            return;
        };
        match self.agent_modes.merge_wire(value) {
            Ok(modes) => self.agent_modes = modes,
            Err(error) => eprintln!("config agent_modes: {error}; using the legacy default"),
        }
    }

    fn apply_router_config(&mut self, config: &Value) {
        let Some(choice) = config
            .get("router_model")
            .cloned()
            .and_then(|value| serde_json::from_value::<ModelChoice>(value).ok())
        else {
            return;
        };
        match crate::router::validate_router_choice(&choice) {
            Ok(()) => self.router_choice = Some(choice),
            Err(error) => eprintln!("config router_model: {error}; using the default"),
        }
    }

    /// What this machine can make, as the account asks it: one bridge serves
    /// one worktrees root, so the first registered project answers for the
    /// account. Whether any project is registered is no fact of a volume, so
    /// the probe never words it and this does.
    pub(in crate::app) fn account_availability(&self) -> IsolationAvailability {
        match self.projects.iter().next() {
            Some(project) => project.orch.worktrees().availability(),
            None => IsolationAvailability::unavailable("no project registered yet"),
        }
    }

    /// How a new checkout of `project_id` is made: the project's own answer, or
    /// the account's when it names none, put to what this volume can actually
    /// make. The one place a setting becomes a decision — nothing else reads
    /// either.
    pub(in crate::app) fn resolved_isolation(&self, project_id: &str) -> ResolvedIsolation {
        let Some(project) = self.projects.get(project_id) else {
            return ResolvedIsolation::honoured(Isolation::default());
        };
        self.decide_isolation(project, &project.orch.worktrees().availability())
    }

    /// The same decision made against an availability the caller already read,
    /// so a project row reports its volume's answer and the isolation that
    /// answer leads to without probing the volume twice.
    pub(in crate::app) fn decide_isolation(
        &self,
        project: &Project,
        available: &IsolationAvailability,
    ) -> ResolvedIsolation {
        let requested = project.isolation.unwrap_or(self.isolation);
        match available.lock_reason(requested) {
            None => ResolvedIsolation::honoured(requested),
            Some(reason) => ResolvedIsolation::downgraded(reason),
        }
    }
}
