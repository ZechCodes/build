//! A model selection and the vocabulary it travels in.
//!
//! What each provider offers — its catalog, its reasoning-effort levels, the
//! flags a selection turns into — belongs to that provider's
//! [`Harness`](crate::harness::Harness), not here. This module holds only what
//! is true of every selection: how one is named on the wire, and the sanity
//! gate every one passes through.
//!
//! Selection is validated but not restricted to the catalog: an unknown id with
//! a safe shape passes through, so a newly released model is usable before a
//! bridge update. Args reach the harness as an exec argv (never a shell string),
//! so validation is a sanity gate, not the injection barrier.

use serde::{Deserialize, Serialize};

use crate::harness::harness_for;

/// The local coding-agent CLI used for an entity's sessions. Persisted on plans
/// and runs so changing a later default never moves existing work to a different
/// harness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider {
    #[default]
    Claude,
    Codex,
    /// The same CLI as [`Claude`](AgentProvider::Claude), run headless over its
    /// session protocol instead of as a TUI. A different provider rather than a
    /// flag on that one: the session it opens has no terminal, and whether a
    /// spawn opens a terminal or a session protocol is what a provider answers.
    #[serde(rename = "claude_adk")]
    ClaudeAdk,
    #[serde(rename = "codex_app_server")]
    CodexAppServer,
    Pi,
}

impl AgentProvider {
    /// Every provider Build can dispatch to. The one place they are enumerated:
    /// anything that has to visit them all — the catalog RPC, the tests that
    /// hold each harness to the same contract — reads this rather than writing
    /// the list out again.
    pub const ALL: [AgentProvider; 5] = [
        AgentProvider::Claude,
        AgentProvider::Codex,
        AgentProvider::ClaudeAdk,
        AgentProvider::CodexAppServer,
        AgentProvider::Pi,
    ];

    /// How a provider is spelled on the wire and in the store. Matches the
    /// serde representation, so a persisted record and an RPC param agree.
    pub fn wire_id(self) -> &'static str {
        match self {
            AgentProvider::Claude => "claude",
            AgentProvider::Codex => "codex",
            AgentProvider::ClaudeAdk => "claude_adk",
            AgentProvider::CodexAppServer => "codex_app_server",
            AgentProvider::Pi => "pi",
        }
    }

    /// The provider a client named, or `None` if this bridge has no such
    /// harness.
    pub fn from_wire(id: &str) -> Option<AgentProvider> {
        AgentProvider::ALL
            .into_iter()
            .find(|provider| provider.wire_id() == id)
    }

    /// What a human sees this provider called.
    pub fn label(self) -> &'static str {
        harness_for(self).label()
    }
}

/// The two words the older `claude_mode` setting spoke, and the carriers they
/// name. Kept as a compat alias, not as a second vocabulary: the account
/// setting is a provider token now, and this is only how a client or config
/// file written before that still says the same thing.
///
/// A mode this bridge has no carrier for answers `None`.
pub fn carrier_of_claude_mode(mode: &str) -> Option<AgentProvider> {
    carrier_of_mode(mode, AgentProvider::ClaudeAdk, AgentProvider::Claude)
}

/// The same mapping backwards, for the old key `settings.get` keeps serving.
/// Only the terminal provider is "tui"; every other default is the honest "not
/// tui", which is also what the old key defaulted to.
pub fn claude_mode_of_harness(harness: AgentProvider) -> &'static str {
    mode_of_harness(harness, AgentProvider::Claude)
}

pub fn carrier_of_codex_mode(mode: &str) -> Option<AgentProvider> {
    carrier_of_mode(mode, AgentProvider::CodexAppServer, AgentProvider::Codex)
}

pub fn codex_mode_of_harness(harness: AgentProvider) -> &'static str {
    mode_of_harness(harness, AgentProvider::Codex)
}

/// The two terminal-mode defaults inferred from the pre-`agent_modes`
/// provider setting. Kept beside the other provider compatibility mappings so
/// provider identity never becomes a second harness dispatch above this layer.
pub(crate) fn legacy_agent_modes_are_tui(harness: AgentProvider) -> (bool, bool) {
    (
        harness == AgentProvider::Claude,
        harness != AgentProvider::CodexAppServer,
    )
}

fn carrier_of_mode(
    mode: &str,
    headless: AgentProvider,
    tui: AgentProvider,
) -> Option<AgentProvider> {
    match mode {
        "headless" => Some(headless),
        "tui" => Some(tui),
        _ => None,
    }
}

fn mode_of_harness(harness: AgentProvider, tui: AgentProvider) -> &'static str {
    if harness == tui {
        "tui"
    } else {
        "headless"
    }
}

/// One selectable model.
#[derive(Debug, Clone, Serialize)]
pub struct ModelOption {
    /// The id passed to `claude --model`.
    pub id: &'static str,
    /// Human label for selectors.
    pub label: &'static str,
    /// Whether `--effort` may be combined with this model.
    pub supports_effort: bool,
    /// Exact effort values this model accepts. Empty means effort is disabled.
    pub efforts: &'static [&'static str],
    /// The most tokens this model holds in context, where it is documented.
    /// Absent for a model whose window Build does not know.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// The context window of the model an agent runs, when its harness's catalog
/// knows it.
///
/// A running model reports its id as the catalog spells it or with something
/// after it — `claude-opus-5[1m]` — so the longest catalog id the model starts
/// with is the one it is. An alias (`opus`) is no id, and knows no window.
pub fn context_window_of(provider: AgentProvider, model: &str) -> Option<u64> {
    harness_for(provider)
        .models()
        .into_iter()
        .filter(|option| model.starts_with(option.id))
        .max_by_key(|option| option.id.len())
        .and_then(|option| option.context_window)
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderCatalog {
    pub id: AgentProvider,
    pub label: &'static str,
    pub models: Vec<ModelOption>,
    pub efforts: &'static [&'static str],
    /// The program this harness runs, as it is looked up on `PATH`.
    pub binary: &'static str,
    /// Whether that program is on this machine's `PATH` right now.
    ///
    /// Advertising every harness whether or not it can run is how an agent
    /// picks one that fails at spawn, minutes later, with nothing to say about
    /// why. The same question `isolation_available` answers for checkouts.
    pub installed: bool,
}

/// Every provider's catalog, for the picker that has to show them all.
pub fn provider_catalogs() -> Vec<ProviderCatalog> {
    AgentProvider::ALL
        .into_iter()
        .map(|provider| {
            let harness = harness_for(provider);
            ProviderCatalog {
                id: provider,
                label: harness.label(),
                models: harness.models(),
                efforts: harness.effort_levels(),
                binary: harness.binary(),
                installed: binary_is_on_path(harness.binary()),
            }
        })
        .collect()
}

/// Whether a program can be found on `PATH`.
///
/// Answered once per program and remembered. This is asked every time a tool
/// list is built — which is every session open — and a harness that was
/// installed a moment ago is not going to be uninstalled between two of them.
/// A bridge restart asks again, which is when the answer could have changed.
///
/// No execution: existence and the executable bit, nothing run. Probing by
/// running `--version` is what the codex harness does to read a version, and
/// it costs a process per ask; this question does not need one.
pub fn binary_is_on_path(program: &str) -> bool {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    if let Some(found) = seen.lock().ok().and_then(|seen| seen.get(program).copied()) {
        return found;
    }
    let found = std::env::var_os("PATH")
        .map(|path| {
            std::env::split_paths(&path).any(|directory| {
                let candidate = directory.join(program);
                std::fs::metadata(&candidate).is_ok_and(|found| found.is_file())
            })
        })
        .unwrap_or(false);
    if let Ok(mut seen) = seen.lock() {
        seen.insert(program.to_string(), found);
    }
    found
}

/// An agent's model selection (chosen at plan or run dispatch). `None` means
/// the selected provider's configured default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    #[serde(default)]
    pub provider: AgentProvider,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl ModelChoice {
    /// Validate a selection: efforts come from the fixed set; model ids must be
    /// shell-sane (defense in depth) and, when the model is in the catalog and
    /// declares no effort support, an effort is rejected rather than passed to
    /// a harness that will 400.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(model) = &self.model {
            // First char must be alphanumeric so an id can never be parsed
            // as a flag by the harness (`--model --effort` style confusion).
            let sane = model.len() <= 64
                && model
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphanumeric())
                && model
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'));
            if !sane {
                return Err(format!("invalid model id: {model:?}"));
            }
        }
        if let Some(effort) = &self.effort {
            let harness = harness_for(self.provider);
            let allowed = harness.effort_levels();
            if !allowed.contains(&effort.as_str()) {
                return Err(format!(
                    "invalid effort {effort:?} (expected one of {})",
                    allowed.join("/")
                ));
            }
            if let Some(model) = &self.model {
                if let Some(entry) = harness.models().iter().find(|m| m.id == model) {
                    if !entry.efforts.contains(&effort.as_str()) {
                        return Err(format!("model {model} does not support effort {effort}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// The argv fragment this selection becomes, in the flags its own provider
    /// spells them with.
    pub fn harness_args(&self) -> Vec<String> {
        harness_for(self.provider).model_args(self)
    }
}

/// What this device says a project agent starts on.
///
/// Every field may be unsaid, and each unsaid one means a default that already
/// exists: no provider is the device's default harness, no model or effort is
/// the harness's own. So a device that has chosen nothing changes nothing,
/// which is what lets the setting be answered before anyone has visited it.
///
/// A project agent talks ABOUT a project rather than working in a checkout, and
/// the harness that suits that job is often not the one coding work leads with
/// — which is why it is a setting of its own rather than a second reading of
/// `default_harness`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectAgentChoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl ProjectAgentChoice {
    /// Whether this cell names anything at all. An empty cell is not written
    /// to the config and not sent on the wire: it is the absence of a choice,
    /// and a grid full of `{}` would be noise in a file a human reads.
    pub fn says_nothing(&self) -> bool {
        self.provider.is_none() && self.model.is_none() && self.effort.is_none()
    }

    /// The concrete selection a mint spends: what this device said, with
    /// `default` — the device's default harness — standing where it said
    /// nothing about the harness.
    pub fn resolved(&self, default: AgentProvider) -> ModelChoice {
        ModelChoice {
            provider: self.provider.unwrap_or(default),
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }
}

/// What an agent is being made to be. The user declares which of these each
/// model can fill, and an agent making an agent asks for one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    Planner,
    Implementer,
    Reviewer,
    Executor,
}

impl AgentRole {
    pub const ALL: [AgentRole; 4] = [
        AgentRole::Planner,
        AgentRole::Implementer,
        AgentRole::Reviewer,
        AgentRole::Executor,
    ];

    pub fn wire_id(self) -> &'static str {
        match self {
            AgentRole::Planner => "planner",
            AgentRole::Implementer => "implementer",
            AgentRole::Reviewer => "reviewer",
            AgentRole::Executor => "executor",
        }
    }

    pub fn describes(self) -> &'static str {
        match self {
            AgentRole::Planner => "works out what to do and how to split it",
            AgentRole::Implementer => "writes and changes the code",
            AgentRole::Reviewer => "reads a change and says what is wrong with it",
            AgentRole::Executor => "carries out a plan that already exists, step by step",
        }
    }

    pub fn from_wire(word: &str) -> Option<AgentRole> {
        AgentRole::ALL
            .into_iter()
            .find(|role| role.wire_id() == word.trim())
    }
}

/// How much direction a model needs from whoever is handing it work.
///
/// An output of the choice, not an input to it: the orchestrator asks for a
/// reviewer, is told which model it got and how much direction that model
/// wants, and writes the brief accordingly. A one-line brief to a
/// step-by-step model is the mistake this exists to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    Generalist,
    Scoped,
    StepByStep,
}

impl AgentCapability {
    pub const ALL: [AgentCapability; 3] = [
        AgentCapability::Generalist,
        AgentCapability::Scoped,
        AgentCapability::StepByStep,
    ];

    pub fn wire_id(self) -> &'static str {
        match self {
            AgentCapability::Generalist => "generalist",
            AgentCapability::Scoped => "scoped",
            AgentCapability::StepByStep => "step_by_step",
        }
    }

    /// What to DO about it, said as the instruction it is. This is what the
    /// creating agent reads off the answer.
    pub fn describes(self) -> &'static str {
        match self {
            AgentCapability::Generalist => {
                "give it the goal and let it work out the rest; it needs little direction"
            }
            AgentCapability::Scoped => {
                "give it a clear scope and the constraints, then leave it to the how"
            }
            AgentCapability::StepByStep => {
                "give it the steps; it does what it is told well and infers little"
            }
        }
    }

    pub fn from_wire(word: &str) -> Option<AgentCapability> {
        AgentCapability::ALL
            .into_iter()
            .find(|capability| capability.wire_id() == word.trim())
    }
}

/// One model the user has declared, and what they have declared it for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoleModel {
    /// Which harness runs it. Absent means the device's default harness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AgentProvider>,
    pub model: String,
    /// The roles this model can fill. A model declared for none is in the list
    /// but never chosen by role, which is a legible thing to want: it stays
    /// there, dimmed, rather than having to be deleted and typed again.
    #[serde(default)]
    pub roles: Vec<AgentRole>,
    pub capability: AgentCapability,
}

/// What this device says its models are for (spec: Agent roles).
///
/// A list and not a map, because the ORDER is the user's preference: when two
/// models can both review, the one they put first is the one that reviews.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RoleModels(pub Vec<RoleModel>);

impl RoleModels {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The model this device would use for a role, and how much direction it
    /// wants. `None` when the user has declared nothing for that role — and
    /// then the caller's own choice, or the harness's default, stands.
    ///
    /// First match wins, because the list is in the user's preference order.
    /// A `capability` narrows it: an orchestrator that knows it can only write
    /// a one-line brief can ask for a generalist and be told plainly that
    /// there is not one.
    pub fn for_role(
        &self,
        role: AgentRole,
        capability: Option<AgentCapability>,
    ) -> Option<&RoleModel> {
        self.0.iter().find(|entry| {
            entry.roles.contains(&role)
                && capability.is_none_or(|wanted| entry.capability == wanted)
        })
    }

    /// Every role any declared model can fill, for a refusal that has to say
    /// what IS on offer.
    pub fn roles_offered(&self) -> Vec<AgentRole> {
        let mut offered = Vec::new();
        for entry in &self.0 {
            for role in &entry.roles {
                if !offered.contains(role) {
                    offered.push(*role);
                }
            }
        }
        offered.sort();
        offered
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_claude_model_but_haiku_has_a_million_token_window() {
        for model in harness_for(AgentProvider::Claude).models() {
            let expected = if model.id.contains("haiku") {
                200_000
            } else {
                1_000_000
            };
            assert_eq!(model.context_window, Some(expected), "{}", model.id);
        }
    }

    #[test]
    fn a_models_window_is_found_by_its_id_or_a_running_variant_of_it() {
        let claude = AgentProvider::Claude;
        assert_eq!(context_window_of(claude, "claude-opus-5"), Some(1_000_000));
        assert_eq!(
            context_window_of(claude, "claude-opus-5[1m]"),
            Some(1_000_000),
            "a running model reports its id with a suffix"
        );
        assert_eq!(context_window_of(claude, "opus"), None);
        assert_eq!(context_window_of(AgentProvider::Pi, "anything"), None);
    }

    fn choice(model: Option<&str>, effort: Option<&str>) -> ModelChoice {
        ModelChoice {
            provider: AgentProvider::Claude,
            model: model.map(str::to_string),
            effort: effort.map(str::to_string),
        }
    }

    #[test]
    fn default_choice_is_harness_default_and_adds_no_args() {
        let none = ModelChoice::default();
        assert!(none.validate().is_ok());
        assert!(none.harness_args().is_empty());
    }

    #[test]
    fn catalog_model_with_effort_validates_and_builds_args() {
        let c = choice(Some("claude-opus-4-8"), Some("xhigh"));
        assert!(c.validate().is_ok());
        assert_eq!(
            c.harness_args(),
            vec!["--model", "claude-opus-4-8", "--effort", "xhigh"]
        );
    }

    #[test]
    fn validate_does_not_gate_on_the_catalog() {
        assert!(choice(Some("a-model-no-catalog-names"), Some("high"))
            .validate()
            .is_ok());
        assert!(choice(Some("claude-fable-5"), Some("max"))
            .validate()
            .is_ok());
        assert!(choice(Some("claude-fable-5"), None).validate().is_ok());
    }

    #[test]
    fn unsafe_model_ids_are_rejected() {
        for bad in ["", "a b", "x;rm -rf /", "--model", "m\n", &"x".repeat(65)] {
            assert!(choice(Some(bad), None).validate().is_err(), "{bad:?}");
        }
    }

    #[test]
    fn effort_must_come_from_the_providers_own_set() {
        // "ultra" is a Codex level; a Claude selection must not take it.
        assert!(choice(None, Some("ultra")).validate().is_err());
        for level in harness_for(AgentProvider::Claude).effort_levels() {
            assert!(choice(None, Some(level)).validate().is_ok(), "{level}");
        }
    }

    #[test]
    fn effort_on_a_no_effort_catalog_model_is_rejected() {
        assert!(choice(Some("claude-haiku-4-5"), Some("high"))
            .validate()
            .is_err());
        assert!(choice(Some("claude-haiku-4-5"), None).validate().is_ok());
    }

    /// The wire spelling is the serde spelling. A record persisted through
    /// serde and a provider named in an RPC param have to be the same string,
    /// or a reloaded plan dispatches to a harness the client cannot ask for.
    #[test]
    fn a_provider_round_trips_through_its_wire_id() {
        for provider in AgentProvider::ALL {
            let id = provider.wire_id();
            assert_eq!(AgentProvider::from_wire(id), Some(provider));
            assert_eq!(
                serde_json::to_value(provider).unwrap(),
                serde_json::Value::String(id.to_string())
            );
        }
        assert_eq!(AgentProvider::from_wire("adk"), None);
    }

    #[test]
    fn pi_round_trips_with_an_empty_catalog_and_its_thinking_levels() {
        assert_eq!(AgentProvider::Pi.wire_id(), "pi");
        assert_eq!(AgentProvider::from_wire("pi"), Some(AgentProvider::Pi));
        let pi = provider_catalogs()
            .into_iter()
            .find(|catalog| catalog.id == AgentProvider::Pi)
            .expect("Pi is advertised");
        assert_eq!(pi.label, "Pi");
        assert!(pi.models.is_empty());
        assert_eq!(
            pi.efforts,
            &["off", "minimal", "low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn provider_catalogs_expose_codex_models_and_model_specific_efforts() {
        let providers = provider_catalogs();
        let claude = providers
            .iter()
            .find(|provider| provider.id == AgentProvider::Claude)
            .unwrap();
        let codex = providers
            .iter()
            .find(|provider| provider.id == AgentProvider::Codex)
            .unwrap();

        assert_eq!(claude.label, "Claude Code TUI");
        assert!(claude
            .models
            .iter()
            .any(|model| model.id == "claude-opus-4-8"));
        let sol = codex
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(codex.label, "Codex TUI");
        assert!(sol.efforts.contains(&"ultra"));
        let luna = codex
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6-luna")
            .unwrap();
        assert!(!luna.efforts.contains(&"ultra"));
    }

    #[test]
    fn astra_is_selectable_on_both_codex_carriers_with_all_reasoning_levels() {
        for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
            let catalog = catalog_of(provider);
            let astra = catalog.models.first().unwrap();
            assert_eq!(astra.id, "gpt-6-astra");
            assert_eq!(astra.label, "GPT-6-Astra");
            assert!(astra.supports_effort);
            assert_eq!(
                astra.efforts,
                &["low", "medium", "high", "xhigh", "max", "ultra"]
            );
            for effort in astra.efforts {
                let choice = ModelChoice {
                    provider,
                    model: Some(astra.id.into()),
                    effort: Some((*effort).into()),
                };
                assert!(choice.validate().is_ok());
            }
        }
    }

    #[test]
    fn codex_choice_maps_reasoning_to_config_instead_of_claude_effort_flag() {
        let c = ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-sol".into()),
            effort: Some("ultra".into()),
        };
        assert!(c.validate().is_ok());
        assert_eq!(
            c.harness_args(),
            vec![
                "--model",
                "gpt-5.6-sol",
                "--config",
                "model_reasoning_effort=\"ultra\""
            ]
        );
    }

    #[test]
    fn codex_rejects_an_effort_the_selected_model_does_not_support() {
        let c = ModelChoice {
            provider: AgentProvider::Codex,
            model: Some("gpt-5.6-luna".into()),
            effort: Some("ultra".into()),
        };
        assert!(c
            .validate()
            .unwrap_err()
            .contains("does not support effort ultra"));
    }

    #[test]
    fn both_codex_carriers_share_one_catalog() {
        let tui = catalog_of(AgentProvider::Codex);
        let app_server = catalog_of(AgentProvider::CodexAppServer);
        assert_eq!(
            tui.models.iter().map(|model| model.id).collect::<Vec<_>>(),
            app_server
                .models
                .iter()
                .map(|model| model.id)
                .collect::<Vec<_>>()
        );
        assert_eq!(tui.efforts, app_server.efforts);
    }

    /// An agent is locked to its harness, so the harnesses sit side by
    /// side and each needs a name of its own. The default carrier owns the
    /// plain name; the word the code uses for the difference stays out of every
    /// label a human reads.
    #[test]
    fn every_catalog_label_a_human_reads_is_distinct_and_free_of_jargon() {
        let labels: Vec<&str> = provider_catalogs()
            .iter()
            .map(|catalog| catalog.label)
            .collect();
        for label in &labels {
            assert!(
                !label.to_lowercase().contains("headless"),
                "{label:?} names the provider the way the code does"
            );
        }
        let distinct: std::collections::BTreeSet<&&str> = labels.iter().collect();
        assert_eq!(distinct.len(), labels.len(), "{labels:?} are not distinct");
        assert_eq!(AgentProvider::ClaudeAdk.label(), "Claude Code");
        assert_eq!(AgentProvider::Claude.label(), "Claude Code TUI");
        assert_eq!(AgentProvider::Codex.label(), "Codex TUI");
        assert_eq!(AgentProvider::CodexAppServer.label(), "Codex");
    }

    #[test]
    fn legacy_codex_and_app_server_are_distinct_persisted_providers() {
        let old: AgentProvider = serde_json::from_str(r#""codex""#).unwrap();
        let app_server: AgentProvider = serde_json::from_str(r#""codex_app_server""#).unwrap();

        assert_eq!(old, AgentProvider::Codex);
        assert_eq!(app_server, AgentProvider::CodexAppServer);
        assert_eq!(serde_json::to_string(&old).unwrap(), r#""codex""#);
        assert_eq!(
            serde_json::to_string(&app_server).unwrap(),
            r#""codex_app_server""#
        );
    }

    const CLAUDE_PROVIDERS: [AgentProvider; 2] = [AgentProvider::Claude, AgentProvider::ClaudeAdk];

    fn catalog_of(provider: AgentProvider) -> ProviderCatalog {
        provider_catalogs()
            .into_iter()
            .find(|catalog| catalog.id == provider)
            .expect("every provider has a catalog")
    }

    fn claude_providers_offer(model_id: &str, label: &str) {
        for provider in CLAUDE_PROVIDERS {
            let catalog = catalog_of(provider);
            let offered = catalog
                .models
                .iter()
                .find(|model| model.id == model_id)
                .unwrap_or_else(|| panic!("{provider:?} does not offer {label}"));
            assert_eq!(offered.label, label);
            assert!(offered.supports_effort);
            assert_eq!(offered.efforts, catalog.efforts);
        }
    }

    /// Both claude providers run the same CLI, so a model released for one is
    /// available on the other by construction.
    #[test]
    fn opus_5_is_in_both_claude_providers_catalogs_with_effort() {
        claude_providers_offer("claude-opus-5", "Claude Opus 5");
    }

    /// Opus 5.5 (announced 2026-09-22) sits directly below Fable 5.1, above
    /// Opus 5, with the same effort levels and window as Opus 5.
    #[test]
    fn opus_5_5_is_in_both_claude_providers_catalogs_below_fable() {
        claude_providers_offer("claude-opus-5-5", "Claude Opus 5.5");
        for provider in CLAUDE_PROVIDERS {
            let ids: Vec<_> = catalog_of(provider)
                .models
                .iter()
                .map(|model| model.id)
                .collect();
            assert_eq!(
                &ids[..3],
                &["claude-fable-5-1", "claude-opus-5-5", "claude-opus-5"]
            );
        }
        assert_eq!(
            context_window_of(AgentProvider::ClaudeAdk, "claude-opus-5-5[1m]"),
            Some(1_000_000)
        );
    }

    /// GPT-6 Sol and Luna (announced 2026-09-22) follow Astra on both codex
    /// carriers; Sol takes every reasoning level as the 5.6 Sol did, Luna
    /// stops at max as the 5.6 Luna did.
    #[test]
    fn gpt_6_sol_and_luna_follow_astra_on_both_codex_carriers() {
        for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
            let catalog = catalog_of(provider);
            let ids: Vec<_> = catalog.models.iter().map(|model| model.id).collect();
            assert_eq!(
                &ids[..4],
                &["gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol"]
            );
            let sol = catalog.models.iter().find(|m| m.id == "gpt-6-sol").unwrap();
            assert_eq!(sol.label, "GPT-6-Sol");
            assert_eq!(
                sol.efforts,
                &["low", "medium", "high", "xhigh", "max", "ultra"]
            );
            let luna = catalog
                .models
                .iter()
                .find(|m| m.id == "gpt-6-luna")
                .unwrap();
            assert_eq!(luna.label, "GPT-6-Luna");
            assert_eq!(luna.efforts, &["low", "medium", "high", "xhigh", "max"]);
            assert!(ModelChoice {
                provider,
                model: Some("gpt-6-sol".into()),
                effort: Some("ultra".into()),
            }
            .validate()
            .is_ok());
            assert!(ModelChoice {
                provider,
                model: Some("gpt-6-luna".into()),
                effort: Some("ultra".into()),
            }
            .validate()
            .unwrap_err()
            .contains("does not support effort ultra"));
        }
    }

    #[test]
    fn fable_5_1_leads_both_claude_providers_catalogs_and_retires_fable_5() {
        claude_providers_offer("claude-fable-5-1", "Claude Fable 5.1");
        for provider in CLAUDE_PROVIDERS {
            assert_eq!(catalog_of(provider).models[0].id, "claude-fable-5-1");
        }
        for catalog in provider_catalogs() {
            assert!(
                !catalog
                    .models
                    .iter()
                    .any(|model| model.id == "claude-fable-5"),
                "{:?} still offers the retired Claude Fable 5",
                catalog.id
            );
        }
    }

    /// The compat alias round trips both ways for the two providers it can
    /// name, and answers the old default for everything else.
    #[test]
    fn the_old_claude_mode_words_map_to_providers_and_back() {
        assert_eq!(carrier_of_claude_mode("tui"), Some(AgentProvider::Claude));
        assert_eq!(
            carrier_of_claude_mode("headless"),
            Some(AgentProvider::ClaudeAdk)
        );
        assert_eq!(carrier_of_claude_mode("codex"), None);
        for provider in [AgentProvider::Claude, AgentProvider::ClaudeAdk] {
            assert_eq!(
                carrier_of_claude_mode(claude_mode_of_harness(provider)),
                Some(provider)
            );
        }
        assert_eq!(claude_mode_of_harness(AgentProvider::Codex), "headless");
    }

    #[test]
    fn the_old_codex_mode_words_map_to_concrete_carriers() {
        assert_eq!(carrier_of_codex_mode("tui"), Some(AgentProvider::Codex));
        assert_eq!(
            carrier_of_codex_mode("headless"),
            Some(AgentProvider::CodexAppServer)
        );
        assert_eq!(carrier_of_codex_mode("unknown"), None);
        assert_eq!(codex_mode_of_harness(AgentProvider::Codex), "tui");
        assert_eq!(
            codex_mode_of_harness(AgentProvider::CodexAppServer),
            "headless"
        );
    }

    #[test]
    fn old_choices_without_a_provider_deserialize_as_claude() {
        let choice: ModelChoice =
            serde_json::from_str(r#"{"model":"claude-opus-4-8","effort":"high"}"#).unwrap();
        assert_eq!(choice.provider, AgentProvider::Claude);
    }
}

#[cfg(test)]
mod role_model_tests {
    use super::*;

    /// The maintainer's own example, written down.
    fn declared() -> RoleModels {
        RoleModels(vec![
            RoleModel {
                provider: None,
                model: "claude-fable-5-1".into(),
                roles: vec![AgentRole::Planner, AgentRole::Reviewer],
                capability: AgentCapability::Generalist,
            },
            RoleModel {
                provider: None,
                model: "claude-opus-5".into(),
                roles: vec![
                    AgentRole::Planner,
                    AgentRole::Reviewer,
                    AgentRole::Implementer,
                ],
                capability: AgentCapability::Scoped,
            },
            RoleModel {
                provider: Some(AgentProvider::Codex),
                model: "gpt-5".into(),
                roles: vec![AgentRole::Implementer, AgentRole::Executor],
                capability: AgentCapability::StepByStep,
            },
        ])
    }

    /// The list is in the user's preference order, so the first model that can
    /// fill a role is the one that does.
    #[test]
    fn the_first_model_that_can_fill_a_role_is_the_one_that_does() {
        let declared = declared();
        let reviewer = declared.for_role(AgentRole::Reviewer, None).unwrap();
        assert_eq!(reviewer.model, "claude-fable-5-1");
        assert_eq!(reviewer.capability, AgentCapability::Generalist);

        // Two models implement; the earlier one wins.
        let implementer = declared.for_role(AgentRole::Implementer, None).unwrap();
        assert_eq!(implementer.model, "claude-opus-5");
    }

    /// A capability narrows it, which is how an orchestrator that can only
    /// write a short brief asks for a model that needs a short brief.
    #[test]
    fn a_capability_narrows_the_choice() {
        let declared = declared();
        let stepwise = declared
            .for_role(AgentRole::Implementer, Some(AgentCapability::StepByStep))
            .unwrap();
        assert_eq!(stepwise.model, "gpt-5");
        assert_eq!(stepwise.provider, Some(AgentProvider::Codex));

        // And asking for one nobody is answers nothing rather than something
        // close enough.
        assert!(declared
            .for_role(AgentRole::Executor, Some(AgentCapability::Generalist))
            .is_none());
    }

    /// A role nobody was declared for answers nothing, and the caller's own
    /// choice stands.
    #[test]
    fn a_role_with_no_model_answers_nothing() {
        let none = RoleModels::default();
        assert!(none.for_role(AgentRole::Planner, None).is_none());
        assert!(none.is_empty());
        assert_eq!(none.roles_offered(), Vec::new());
    }

    /// What IS on offer, for a refusal that has to leave the caller somewhere
    /// to go. Deduplicated: three models that all plan is one role.
    #[test]
    fn the_roles_on_offer_are_said_once_each() {
        assert_eq!(
            declared().roles_offered(),
            vec![
                AgentRole::Planner,
                AgentRole::Implementer,
                AgentRole::Reviewer,
                AgentRole::Executor
            ]
        );
    }

    /// A model declared for no role stays in the list and is never chosen —
    /// which is a legible thing to want, rather than having to delete it.
    #[test]
    fn a_model_with_no_roles_is_kept_and_never_chosen() {
        let parked = RoleModels(vec![RoleModel {
            provider: None,
            model: "claude-haiku-4-5".into(),
            roles: Vec::new(),
            capability: AgentCapability::StepByStep,
        }]);
        assert!(!parked.is_empty());
        assert_eq!(parked.roles_offered(), Vec::new());
        for role in AgentRole::ALL {
            assert!(parked.for_role(role, None).is_none());
        }
    }

    /// Both words round-trip, and each says something a reader can act on —
    /// a capability's words being the instruction the orchestrator follows.
    #[test]
    fn the_words_read_back_and_say_what_to_do() {
        for role in AgentRole::ALL {
            assert_eq!(AgentRole::from_wire(role.wire_id()), Some(role));
            assert!(!role.describes().is_empty());
        }
        for capability in AgentCapability::ALL {
            assert_eq!(
                AgentCapability::from_wire(capability.wire_id()),
                Some(capability)
            );
            assert!(!capability.describes().is_empty());
        }
        assert_eq!(
            AgentCapability::from_wire("step_by_step"),
            Some(AgentCapability::StepByStep)
        );
        assert_eq!(
            AgentRole::from_wire("  reviewer "),
            Some(AgentRole::Reviewer)
        );
        assert_eq!(AgentRole::from_wire("reviewing"), None);
    }

    /// An empty list is not written down.
    #[test]
    fn an_empty_list_serializes_to_an_empty_list() {
        assert_eq!(
            serde_json::to_value(RoleModels::default()).unwrap(),
            serde_json::json!([])
        );
    }
}
