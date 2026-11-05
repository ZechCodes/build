//! Trusted user admission context. It is never deserialized from a wire request.
#[derive(Clone, Debug)]
pub struct SecretPostBinding {
    pub ids: Vec<String>,
    pub device_id: String,
    pub creator: String,
    pub project_id: String,
    pub owner_id: String,
    pub agent_id: String,
    pub conversation_id: String,
    pub generation: String,
    pub draft_id: Option<String>,
    pub attached_at: i64,
}
