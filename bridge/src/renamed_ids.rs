//! Ids minted before issues were renamed tasks (#190).
//!
//! The store rewrote every record to the new prefixes when it upgraded
//! (`store/task_rename.rs`), but an old id lives on where no migration
//! reaches: in the context of an agent that read it before the upgrade, in a
//! message body, in a client's cache. Wherever an id comes IN — a tool's
//! arguments, a verb's params — it is read under its new prefix, so the
//! handler behind it only ever sees, compares and stores the new one.

use serde::{Deserialize, Deserializer};

const RENAMED_PREFIXES: [(&str, &str); 3] = [("issue-", "task-"), ("ic-", "tc-"), ("ie-", "te-")];

/// `issue-…` as `task-…`, a comment's `ic-…` as `tc-…` and an event's `ie-…`
/// as `te-…`. Any other id comes back as it was.
pub fn current_id(id: &str) -> String {
    RENAMED_PREFIXES
        .iter()
        .find_map(|(old, new)| id.strip_prefix(old).map(|rest| format!("{new}{rest}")))
        .unwrap_or_else(|| id.to_string())
}

/// For a verb's param: `#[serde(deserialize_with = "crate::renamed_ids::current")]`.
pub fn current<'de, D: Deserializer<'de>>(deserializer: D) -> Result<String, D::Error> {
    String::deserialize(deserializer).map(|id| current_id(&id))
}

/// The same for a param that may be absent.
pub fn current_optional<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer).map(|id| id.as_deref().map(current_id))
}

#[cfg(test)]
mod tests {
    use super::current_id;

    #[test]
    fn an_old_prefix_reads_as_its_new_one_and_nothing_else_moves() {
        assert_eq!(
            current_id("issue-01M3HPVBKS8JPRKXP3JXM4W6VQ"),
            "task-01M3HPVBKS8JPRKXP3JXM4W6VQ"
        );
        assert_eq!(
            current_id("ic-01M3HPXXE28HXBB5VB2QBHQRXV"),
            "tc-01M3HPXXE28HXBB5VB2QBHQRXV"
        );
        assert_eq!(current_id("ie-7"), "te-7");
        for id in ["task-1", "tc-1", "te-1", "run-1", "agent-1", "issued", ""] {
            assert_eq!(current_id(id), id);
        }
    }
}
