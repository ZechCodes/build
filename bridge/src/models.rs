//! The harness model catalog: which models an entity's agents (a plan's or a
//! run's) can run on, and the reasoning-effort levels the harness accepts.
//!
//! There is no way to enumerate models from the harness itself — the Claude Code
//! CLI has no list command, and the Models API needs API-key credentials the
//! bridge cannot assume (harnesses commonly run on subscription auth). So the
//! catalog is curated here and served to clients over RPC (`models.list`); it
//! ships with bridge updates and the UI never hardcodes model ids.
//!
//! Selection is validated but not restricted to the catalog: an unknown id with
//! a safe shape passes through, so a newly released model is usable before a
//! bridge update. Args reach the harness as an exec argv (never a shell string),
//! so validation is a sanity gate, not the injection barrier.

use serde::{Deserialize, Serialize};

/// Reasoning-effort levels accepted by the harness (`claude --effort`).
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// One selectable model.
#[derive(Debug, Clone, Serialize)]
pub struct ModelOption {
    /// The id passed to `claude --model`.
    pub id: &'static str,
    /// Human label for selectors.
    pub label: &'static str,
    /// Whether `--effort` may be combined with this model.
    pub supports_effort: bool,
}

/// The curated catalog, most capable first. (cached: 2026-07)
pub fn catalog() -> Vec<ModelOption> {
    vec![
        ModelOption {
            id: "claude-fable-5",
            label: "Claude Fable 5",
            supports_effort: true,
        },
        ModelOption {
            id: "claude-opus-4-8",
            label: "Claude Opus 4.8",
            supports_effort: true,
        },
        ModelOption {
            id: "claude-sonnet-5",
            label: "Claude Sonnet 5",
            supports_effort: true,
        },
        ModelOption {
            id: "claude-sonnet-4-6",
            label: "Claude Sonnet 4.6",
            supports_effort: true,
        },
        ModelOption {
            id: "claude-haiku-4-5",
            label: "Claude Haiku 4.5",
            supports_effort: false,
        },
    ]
}

/// An agent's model selection (chosen at plan or run dispatch). `None` means the
/// harness default — the user's own Claude Code configuration decides.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
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
            if !EFFORT_LEVELS.contains(&effort.as_str()) {
                return Err(format!(
                    "invalid effort {effort:?} (expected one of {})",
                    EFFORT_LEVELS.join("/")
                ));
            }
            if let Some(model) = &self.model {
                if let Some(entry) = catalog().iter().find(|m| m.id == model) {
                    if !entry.supports_effort {
                        return Err(format!("model {model} does not support effort levels"));
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
            args.push("--effort".to_string());
            args.push(effort.clone());
        }
        args
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(model: Option<&str>, effort: Option<&str>) -> ModelChoice {
        ModelChoice {
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
}
