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
}

impl AgentProvider {
    /// Every provider Build can dispatch to. The one place they are enumerated:
    /// anything that has to visit them all — the catalog RPC, the tests that
    /// hold each harness to the same contract — reads this rather than writing
    /// the list out again.
    pub const ALL: [AgentProvider; 2] = [AgentProvider::Claude, AgentProvider::Codex];

    /// How a provider is spelled on the wire and in the store. Matches the
    /// serde representation, so a persisted record and an RPC param agree.
    pub fn wire_id(self) -> &'static str {
        match self {
            AgentProvider::Claude => "claude",
            AgentProvider::Codex => "codex",
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

/// An agent's model selection (chosen at plan or run dispatch). `None` means the
/// harness default — the user's own Claude Code configuration decides.
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
    fn unknown_but_sane_model_id_passes_through() {
        // A model released after this bridge build must remain usable.
        assert!(choice(Some("claude-opus-5"), Some("high"))
            .validate()
            .is_ok());
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

        assert_eq!(claude.label, "Claude Code");
        assert!(claude
            .models
            .iter()
            .any(|model| model.id == "claude-opus-4-8"));
        let sol = codex
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6-sol")
            .unwrap();
        assert_eq!(codex.label, "Codex CLI");
        assert!(sol.efforts.contains(&"ultra"));
        let luna = codex
            .models
            .iter()
            .find(|model| model.id == "gpt-5.6-luna")
            .unwrap();
        assert!(!luna.efforts.contains(&"ultra"));
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
    fn old_choices_without_a_provider_deserialize_as_claude() {
        let choice: ModelChoice =
            serde_json::from_str(r#"{"model":"claude-opus-4-8","effort":"high"}"#).unwrap();
        assert_eq!(choice.provider, AgentProvider::Claude);
    }
}
