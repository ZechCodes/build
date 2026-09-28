//! The bridge's wire API, as a version.
//!
//! One number for the whole wire: what `session.hello` and `ping` report, and
//! what a client's declared `api_range` is measured against. Semver, per the
//! wire spec (Part 2): a patch changes nothing on the wire, one minor covers
//! all additive wire changes in a release, and a major removes or reshapes.
//!
//! [`v1`] owns the shape of every verb it serves; [`ApiError`] is the closed
//! set of ways a verb refuses.

pub mod clients;
pub mod v1;

use serde_json::{json, Value};
use std::collections::BTreeSet;

/// The version of the wire API this bridge speaks.
/// 2.0.0 is the task rename (#190): every tracker and plan verb and feature
/// name moved to `tasks.*` or `task.*`.
/// 2.1.0 adds `push.registerKey` and `push.revokeKey` (#200): the notification
/// keys push content is sealed to.
pub const API_VERSION: &str = "2.1.0";

/// Verbs served outside the typed v1 table. Keep this list beside the
/// capability builder so the greeting cannot silently omit a legacy verb.
pub const LEGACY_METHODS: &[&str] = &[
    "agent.attach",
    "agent.interrupt",
    "agent.start",
    "bridge.stats",
    "ping",
    "rtc.close",
    "rtc.ice",
    "rtc.offer",
    "session.hello",
    "term.ack",
    "term.attach",
    "term.close",
    "term.create",
    "term.input",
    "term.list",
    "term.resize",
];

/// Fixture-backed verbs served only by a bridge started in QA mode.
pub const QA_METHODS: &[&str] = &["stream.events", "stream.start", "stream.state"];

/// Cross-verb wire features whose availability cannot be expressed by a
/// single method name. The SPA consumes a subset as shape and behavior gates.
pub const FEATURE_CAPABILITIES: &[&str] = &[
    "agents.names",
    "board.conversationSessions", // Since 1.28.0: a conversation row's own session_started_ms/last_activity_ms.
    "board.usageLimits",
    "bodies.pages", // Since 1.26.0: range on fs.read, git.diff, git.show, git.changeset_diff.
    "branches.finishDelete",
    "changes.bodies",
    "changes.refusedKinds",
    "changes.subscriptions",
    "conversations.settings",
    "diffs.perFile",
    "errors.codes",
    "fs.mediaRawPages", // Since 1.30.0: exact binary ranges for Blob media reads, through 64 MiB.
    "tasks.agentIdentities",
    "tasks.attachmentChunks",
    "tasks.attachments",
    "tasks.commentUserMentions",
    "tasks.commentUserNotifies",
    "tasks.context",
    "tasks.createdUserMentions", // Since 1.27.0: mentions_user on an agent's created event.
    "tasks.doneSinceLeft",
    "tasks.listPaged", // Since 1.25.0: limit/cursor and next_cursor on tasks.list.
    "tasks.unreadCounts", // Since 1.29.0: unread_count on a watched task (tasks.list, tasks.get).
    "tasks.watching",
    "messages.context",
    "messages.fromAgent",
    "messages.taskNotices",
    "params.strict",
    "requests.priority",
    "requests.receipts",
    "settings.projectAgent",
    "settings.roleModels",
    "settings.workspaceLifecycle",
    "thread.attachmentChunks", // Since 1.30.0: offset/length on thread.attachment.
    "threads.newestDeltaPagination",
    "threads.postOperations",
    "workspaces.lifecycle",
    "workspaces.reclaimBranches",
];

/// Everything this bridge can serve on a session, as exact method names and
/// explicit feature names. Sorting and deduplication make the greeting stable.
pub fn capabilities(qa_agent: bool) -> Vec<&'static str> {
    let mut names: BTreeSet<&'static str> = v1::methods().iter().map(|(name, _)| *name).collect();
    names.extend(LEGACY_METHODS.iter().copied());
    names.extend(FEATURE_CAPABILITIES.iter().copied());
    if qa_agent {
        names.extend(QA_METHODS.iter().copied());
    }
    names.into_iter().collect()
}

/// Why a verb refused, as the wire spells it (step 2.4). A closed enum: a new
/// variant joins the next release's minor version, and a client that meets a
/// code it does not know treats it as `internal`.
///
/// `error` on the wire stays the message string in 1.x; the code, whether a
/// retry could succeed, and any structured detail ride beside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    /// No verb by that name on this bridge; `details.method` names it.
    UnknownMethod {
        message: String,
        details: Option<Value>,
    },
    /// The params did not parse into what the verb takes.
    InvalidParams {
        message: String,
        details: Option<Value>,
    },
    /// The entity the params name is not here.
    NotFound {
        message: String,
        details: Option<Value>,
    },
    /// A stale `expected_revision` / `expected_choice_revision`; `details`
    /// carries the current value.
    Conflict {
        message: String,
        details: Option<Value>,
    },
    /// A backend this verb needs is not available (isolation, harness).
    Unavailable {
        message: String,
        details: Option<Value>,
    },
    /// A queue is full; the only retryable refusal.
    Busy {
        message: String,
        details: Option<Value>,
    },
    /// The client asked for an API major this bridge does not serve.
    UnsupportedVersion {
        message: String,
        details: Option<Value>,
    },
    /// A refusal the handler could not name more precisely.
    Internal {
        message: String,
        details: Option<Value>,
    },
}

impl ApiError {
    /// Every code, as a fixture may cite it.
    pub const CODES: [&'static str; 8] = [
        "unknown_method",
        "invalid_params",
        "not_found",
        "conflict",
        "unavailable",
        "busy",
        "unsupported_version",
        "internal",
    ];

    pub fn unknown_method(method: &str) -> ApiError {
        ApiError::UnknownMethod {
            message: format!("unknown method: {method}"),
            details: Some(json!({ "method": method })),
        }
    }

    pub fn invalid_params(message: impl Into<String>) -> ApiError {
        ApiError::InvalidParams {
            message: message.into(),
            details: None,
        }
    }

    pub fn not_found(message: impl Into<String>) -> ApiError {
        ApiError::NotFound {
            message: message.into(),
            details: None,
        }
    }

    pub fn conflict(message: impl Into<String>, details: Option<Value>) -> ApiError {
        ApiError::Conflict {
            message: message.into(),
            details,
        }
    }

    pub fn unavailable(message: impl Into<String>) -> ApiError {
        ApiError::Unavailable {
            message: message.into(),
            details: None,
        }
    }

    pub fn busy(message: impl Into<String>) -> ApiError {
        ApiError::Busy {
            message: message.into(),
            details: None,
        }
    }

    pub fn unsupported_version(message: impl Into<String>) -> ApiError {
        ApiError::UnsupportedVersion {
            message: message.into(),
            details: None,
        }
    }

    pub fn internal(message: impl Into<String>) -> ApiError {
        ApiError::Internal {
            message: message.into(),
            details: None,
        }
    }

    /// The wire code, snake_case.
    pub fn code(&self) -> &'static str {
        match self {
            ApiError::UnknownMethod { .. } => "unknown_method",
            ApiError::InvalidParams { .. } => "invalid_params",
            ApiError::NotFound { .. } => "not_found",
            ApiError::Conflict { .. } => "conflict",
            ApiError::Unavailable { .. } => "unavailable",
            ApiError::Busy { .. } => "busy",
            ApiError::UnsupportedVersion { .. } => "unsupported_version",
            ApiError::Internal { .. } => "internal",
        }
    }

    /// Whether the same request could succeed if sent again.
    pub fn retryable(&self) -> bool {
        matches!(self, ApiError::Busy { .. })
    }

    fn parts(&self) -> (&str, Option<&Value>) {
        match self {
            ApiError::UnknownMethod { message, details }
            | ApiError::InvalidParams { message, details }
            | ApiError::NotFound { message, details }
            | ApiError::Conflict { message, details }
            | ApiError::Unavailable { message, details }
            | ApiError::Busy { message, details }
            | ApiError::UnsupportedVersion { message, details }
            | ApiError::Internal { message, details } => (message, details.as_ref()),
        }
    }

    /// The human-readable refusal — the `error` string of 1.x.
    pub fn message(&self) -> &str {
        self.parts().0
    }

    /// Structured detail, when the code has any.
    pub fn details(&self) -> Option<&Value> {
        self.parts().1
    }

    /// The reply envelope for a refused request: the 1.0 `error` string, with
    /// the code, retryability and details beside it (additive in 1.1).
    pub fn into_reply(self, id: Value) -> Value {
        let mut reply = json!({
            "id": id,
            "ok": false,
            "error": self.message(),
            "error_code": self.code(),
            "retryable": self.retryable(),
        });
        if let Some(details) = self.details() {
            reply["details"] = details.clone();
        }
        reply
    }

    /// Name a code for a handler's bare `Err(String)`, from the messages the
    /// handlers already use: a param the verb needed (`missing required
    /// param`, `must`, `cannot be`), an entity that is not here (`unknown
    /// <thing>`), and `internal` for everything else — which a test counts,
    /// so the list shrinks over time.
    pub fn classify(message: String) -> ApiError {
        let sentence = Self::unlabelled(&message);
        if sentence.starts_with("missing required param")
            || sentence.starts_with("missing scope")
            || sentence.starts_with("provide exactly one of")
            || sentence.contains(" must ")
            || sentence.contains(" cannot be ")
        {
            return ApiError::invalid_params(message);
        }
        if sentence.starts_with("unknown ") && !sentence.starts_with("unknown method") {
            return ApiError::not_found(message);
        }
        if sentence.starts_with(crate::github::REFUSAL) {
            return ApiError::unavailable(message);
        }
        // A refusal a deferred verb decided under the lock after its work ran
        // off it: `workspace.reclaim` reads its holds again once Git is
        // measured.
        if sentence.starts_with(crate::reclaim::REFUSAL) {
            return ApiError::conflict(message, None);
        }
        if sentence == crate::reclaim::BUSY || sentence == crate::reclaim::RESERVED {
            return ApiError::busy(message);
        }
        ApiError::internal(message)
    }

    /// The sentence behind a verb's own label: some handlers write
    /// `entity.mute: unknown entity run-7`, and the label is not the refusal.
    fn unlabelled(message: &str) -> &str {
        match message.split_once(": ") {
            Some((label, rest))
                if label.contains('.') && !label.contains(' ') && !rest.is_empty() =>
            {
                rest
            }
            _ => message,
        }
    }
}

impl From<String> for ApiError {
    fn from(message: String) -> ApiError {
        ApiError::internal(message)
    }
}

/// One reply envelope for both outcomes, so the success shape and the refusal
/// shape are written in exactly one place.
pub fn reply(id: Value, result: Result<Value, ApiError>) -> Value {
    match result {
        Ok(result) => json!({ "id": id, "ok": true, "result": result }),
        Err(error) => error.into_reply(id),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_api_version_is_three_dot_separated_integers() {
        let parts: Vec<&str> = API_VERSION.split('.').collect();
        assert_eq!(parts.len(), 3, "{API_VERSION}");
        for part in parts {
            assert!(
                part.parse::<u64>().is_ok(),
                "{part:?} in {API_VERSION} is not an integer"
            );
        }
    }

    #[test]
    fn every_code_is_snake_case_and_only_busy_is_retryable() {
        let errors = [
            ApiError::unknown_method("x"),
            ApiError::invalid_params("m"),
            ApiError::not_found("m"),
            ApiError::conflict("m", None),
            ApiError::unavailable("m"),
            ApiError::busy("m"),
            ApiError::unsupported_version("m"),
            ApiError::internal("m"),
        ];
        for (error, code) in errors.iter().zip(ApiError::CODES) {
            assert_eq!(error.code(), code);
            assert_eq!(error.retryable(), code == "busy");
        }
    }

    #[test]
    fn the_refusal_envelope_keeps_the_string_and_adds_the_code() {
        let reply = ApiError::unknown_method("changes.subscribe").into_reply(json!("r12"));
        assert_eq!(
            reply,
            json!({
                "id": "r12", "ok": false,
                "error": "unknown method: changes.subscribe",
                "error_code": "unknown_method", "retryable": false,
                "details": { "method": "changes.subscribe" }
            })
        );
        let bare = ApiError::internal("boom").into_reply(json!(1));
        assert!(bare.get("details").is_none());
        assert_eq!(bare["error_code"], "internal");
    }

    #[test]
    fn classify_reads_past_a_verb_label_and_names_a_scope_complaint() {
        assert_eq!(
            ApiError::classify("entity.mute: unknown entity run-7".into()).code(),
            "not_found"
        );
        assert_eq!(
            ApiError::classify(
                "provide exactly one of project_id, run_id, or project_id + worktree_id".into()
            )
            .code(),
            "invalid_params"
        );
        // A label is a verb name, not any prefix: a sentence with a colon in it
        // is still read whole.
        assert_eq!(
            ApiError::classify("git switch failed: unknown ref".into()).code(),
            "internal"
        );
    }

    #[test]
    fn a_bare_string_error_is_internal_and_classify_names_the_common_ones() {
        assert_eq!(ApiError::from("boom".to_string()).code(), "internal");
        assert_eq!(
            ApiError::classify("missing required param: paths".into()).code(),
            "invalid_params"
        );
        assert_eq!(
            ApiError::classify("unknown project_id".into()).code(),
            "not_found"
        );
        assert_eq!(
            ApiError::classify("nothing staged to commit".into()).code(),
            "internal"
        );
    }
}
