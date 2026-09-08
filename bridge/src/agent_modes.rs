use serde::Serialize;
use serde_json::Value;

use crate::models::AgentProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    Headless,
    Tui,
}

impl AgentMode {
    fn from_wire(value: &Value, family: &str) -> Result<Self, String> {
        match value.as_str() {
            Some("headless") => Ok(Self::Headless),
            Some("tui") => Ok(Self::Tui),
            Some(named) => Err(format!(
                "unknown agent_modes.{family} {named:?} (expected \"headless\" or \"tui\")"
            )),
            None => Err(format!(
                "agent_modes.{family} must be \"headless\" or \"tui\""
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentModes {
    pub claude: AgentMode,
    pub codex: AgentMode,
}

impl AgentModes {
    pub fn from_legacy_default(default_harness: AgentProvider) -> Self {
        Self {
            claude: if default_harness == AgentProvider::Claude {
                AgentMode::Tui
            } else {
                AgentMode::Headless
            },
            codex: if default_harness == AgentProvider::CodexAppServer {
                AgentMode::Headless
            } else {
                AgentMode::Tui
            },
        }
    }

    pub fn merge_wire(self, value: &Value) -> Result<Self, String> {
        let object = value
            .as_object()
            .ok_or_else(|| "agent_modes must be an object".to_string())?;
        if object.is_empty() {
            return Err("agent_modes must name at least one family".to_string());
        }
        for family in object.keys() {
            if family != "claude" && family != "codex" {
                return Err(format!(
                    "unknown agent_modes family {family:?} (expected \"claude\" or \"codex\")"
                ));
            }
        }
        Ok(Self {
            claude: object
                .get("claude")
                .map(|value| AgentMode::from_wire(value, "claude"))
                .transpose()?
                .unwrap_or(self.claude),
            codex: object
                .get("codex")
                .map(|value| AgentMode::from_wire(value, "codex"))
                .transpose()?
                .unwrap_or(self.codex),
        })
    }
}
