//! The harness model catalog: which models an entity's agents (a plan's or a
//! run's) can run on, and the reasoning-effort levels the harness accepts.
//!
//! Catalogs are curated here and served to clients over RPC (`models.list`), so
//! the UI never hardcodes provider models. Codex has an experimental debug
//! catalog command, but Build cannot assume every installed CLI version exposes
//! it; shipping the catalog keeps the web contract deterministic.
//!
//! Selection is validated but not restricted to the catalog: an unknown id with
//! a safe shape passes through, so a newly released model is usable before a
//! bridge update. Args reach the harness as an exec argv (never a shell string),
//! so validation is a sanity gate, not the injection barrier.

use serde::{Deserialize, Serialize};

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
    pub fn label(self) -> &'static str {
        match self {
            AgentProvider::Claude => "Claude Code",
            AgentProvider::Codex => "Codex CLI",
        }
    }
}

/// Reasoning-effort levels accepted by the harness (`claude --effort`).
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
pub const CODEX_EFFORT_LEVELS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

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

/// The curated catalog, most capable first. (cached: 2026-07)
pub fn catalog() -> Vec<ModelOption> {
    vec![
        ModelOption {
            id: "claude-fable-5",
            label: "Claude Fable 5",
            supports_effort: true,
            efforts: &EFFORT_LEVELS,
        },
        ModelOption {
            id: "claude-opus-4-8",
            label: "Claude Opus 4.8",
            supports_effort: true,
            efforts: &EFFORT_LEVELS,
        },
        ModelOption {
            id: "claude-sonnet-5",
            label: "Claude Sonnet 5",
            supports_effort: true,
            efforts: &EFFORT_LEVELS,
        },
        ModelOption {
            id: "claude-sonnet-4-6",
            label: "Claude Sonnet 4.6",
            supports_effort: true,
            efforts: &EFFORT_LEVELS,
        },
        ModelOption {
            id: "claude-haiku-4-5",
            label: "Claude Haiku 4.5",
            supports_effort: false,
            efforts: &[],
        },
    ]
}

pub fn codex_catalog() -> Vec<ModelOption> {
    const THROUGH_XHIGH: &[&str] = &["low", "medium", "high", "xhigh"];
    const THROUGH_MAX: &[&str] = &["low", "medium", "high", "xhigh", "max"];
    vec![
        ModelOption {
            id: "gpt-5.6-sol",
            label: "GPT-5.6-Sol",
            supports_effort: true,
            efforts: &CODEX_EFFORT_LEVELS,
        },
        ModelOption {
            id: "gpt-5.6-terra",
            label: "GPT-5.6-Terra",
            supports_effort: true,
            efforts: &CODEX_EFFORT_LEVELS,
        },
        ModelOption {
            id: "gpt-5.6-luna",
            label: "GPT-5.6-Luna",
            supports_effort: true,
            efforts: THROUGH_MAX,
        },
        ModelOption {
            id: "gpt-5.5",
            label: "GPT-5.5",
            supports_effort: true,
            efforts: THROUGH_XHIGH,
        },
        ModelOption {
            id: "gpt-5.2",
            label: "GPT-5.2",
            supports_effort: true,
            efforts: THROUGH_XHIGH,
        },
    ]
}

pub fn provider_catalogs() -> Vec<ProviderCatalog> {
    vec![
        ProviderCatalog {
            id: AgentProvider::Claude,
            label: AgentProvider::Claude.label(),
            models: catalog(),
            efforts: &EFFORT_LEVELS,
        },
        ProviderCatalog {
            id: AgentProvider::Codex,
            label: AgentProvider::Codex.label(),
            models: codex_catalog(),
            efforts: &CODEX_EFFORT_LEVELS,
        },
    ]
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
            let allowed = match self.provider {
                AgentProvider::Claude => EFFORT_LEVELS.as_slice(),
                AgentProvider::Codex => CODEX_EFFORT_LEVELS.as_slice(),
            };
            if !allowed.contains(&effort.as_str()) {
                return Err(format!(
                    "invalid effort {effort:?} (expected one of {})",
                    allowed.join("/")
                ));
            }
            if let Some(model) = &self.model {
                let catalog = match self.provider {
                    AgentProvider::Claude => catalog(),
                    AgentProvider::Codex => codex_catalog(),
                };
                if let Some(entry) = catalog.iter().find(|m| m.id == model) {
                    if !entry.efforts.contains(&effort.as_str()) {
                        return Err(format!("model {model} does not support effort {effort}"));
                    }
                }
            }
        }
        Ok(())
    }

    /// The harness argv fragment for this selection.
    pub fn harness_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(model) = &self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(effort) = &self.effort {
            match self.provider {
                AgentProvider::Claude => {
                    args.push("--effort".to_string());
                    args.push(effort.clone());
                }
                AgentProvider::Codex => {
                    args.push("--config".to_string());
                    args.push(format!(
                        "model_reasoning_effort={}",
                        serde_json::to_string(effort).expect("effort serializes")
                    ));
                }
            }
        }
        args
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
    fn effort_must_come_from_the_fixed_set() {
        assert!(choice(None, Some("ultra")).validate().is_err());
        for level in EFFORT_LEVELS {
            assert!(choice(None, Some(level)).validate().is_ok());
        }
    }

    #[test]
    fn effort_on_a_no_effort_catalog_model_is_rejected() {
        assert!(choice(Some("claude-haiku-4-5"), Some("high"))
            .validate()
            .is_err());
        assert!(choice(Some("claude-haiku-4-5"), None).validate().is_ok());
    }

    #[test]
    fn catalog_is_nonempty_and_ids_are_sane() {
        let cat = catalog();
        assert!(!cat.is_empty());
        for m in cat {
            assert!(choice(Some(m.id), None).validate().is_ok(), "{}", m.id);
        }
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
