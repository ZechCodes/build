use serde::{Serialize, Serializer};

use super::bounds::{bounded, GOAL_OBJECTIVE_LIMIT, PROVIDER_TOKEN_LIMIT};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalState {
    Active,
    Paused,
    Blocked,
    UsageLimited,
    BudgetLimited,
    Complete,
    Unknown(String),
}

impl GoalState {
    pub fn from_provider(token: &str) -> Self {
        Self::from_provider_bounded(token).0
    }
    pub fn from_provider_bounded(token: &str) -> (Self, bool) {
        match token {
            "active" => (Self::Active, false),
            "paused" => (Self::Paused, false),
            "blocked" => (Self::Blocked, false),
            "usage_limited" | "usageLimited" => (Self::UsageLimited, false),
            "budget_limited" | "budgetLimited" => (Self::BudgetLimited, false),
            "complete" => (Self::Complete, false),
            token => {
                let (token, cut) = bounded(token, PROVIDER_TOKEN_LIMIT);
                (Self::Unknown(token), cut)
            }
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Blocked => "blocked",
            Self::UsageLimited => "usage_limited",
            Self::BudgetLimited => "budget_limited",
            Self::Complete => "complete",
            Self::Unknown(token) => token,
        }
    }
}

impl Serialize for GoalState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SurfaceGoal {
    pub objective: String,
    pub state: GoalState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_budget: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens_used: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_used_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<i64>,
}

impl SurfaceGoal {
    pub fn new(objective: impl Into<String>, state: GoalState) -> (Self, bool) {
        let (objective, truncated) = bounded(objective, GOAL_OBJECTIVE_LIMIT);
        let (state, state_truncated) = match state {
            GoalState::Unknown(token) => {
                let (token, cut) = bounded(token, PROVIDER_TOKEN_LIMIT);
                (GoalState::Unknown(token), cut)
            }
            state => (state, false),
        };
        (
            Self {
                objective,
                state,
                token_budget: None,
                tokens_used: None,
                time_used_seconds: None,
                created_at: None,
                updated_at: None,
            },
            truncated || state_truncated,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_goal_state_preserves_a_bounded_provider_token() {
        let token = "x".repeat(PROVIDER_TOKEN_LIMIT + 1);
        let (state, truncated) = GoalState::from_provider_bounded(&token);
        assert!(truncated);
        assert_eq!(state.as_str().len(), PROVIDER_TOKEN_LIMIT);
    }

    #[test]
    fn unrecognized_goal_state_is_serialized_as_its_original_token() {
        let state = GoalState::from_provider("completed");
        assert_eq!(state, GoalState::Unknown("completed".to_string()));
        assert_eq!(
            serde_json::to_value(state).unwrap(),
            serde_json::json!("completed")
        );
    }

    #[test]
    fn objective_bound_does_not_split_utf8() {
        let objective = format!("{}é", "x".repeat(GOAL_OBJECTIVE_LIMIT - 1));
        let (goal, truncated) = SurfaceGoal::new(objective, GoalState::Active);
        assert!(truncated);
        assert_eq!(goal.objective.len(), GOAL_OBJECTIVE_LIMIT - 1);
    }
}
