use crate::templates::{fill, MESSAGE_ATTACHED_SECRETS};
use crate::thread::{MessageSecret, SecretEnvironment};

pub(super) fn text(
    environment: SecretEnvironment,
    descriptors: &[MessageSecret],
) -> Option<String> {
    if descriptors.is_empty() {
        return None;
    }
    let rows = descriptors
        .iter()
        .map(|descriptor| serde_json::to_string(descriptor).expect("secret metadata serializes"))
        .collect::<Vec<_>>()
        .join("\n");
    Some(fill(
        MESSAGE_ATTACHED_SECRETS,
        &[
            ("descriptors", &rows),
            ("environment", environment.as_str()),
        ],
    ))
}
