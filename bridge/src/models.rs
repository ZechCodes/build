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
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderCatalog {
    pub id: AgentProvider,
    pub label: &'static str,
    pub models: Vec<ModelOption>,
    pub efforts: &'static [&'static str],
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
            }
        })
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;

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
