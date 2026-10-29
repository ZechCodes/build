//! The changes family: `changes.subscribe` and `changes.unsubscribe` (wire
//! spec Part 1, step 1.1).
//!
//! The subscription belongs to the explicitly supplied application caller.
//! Synchronous dispatch without a caller refuses `unavailable`; asynchronous
//! work never inherits caller identity from thread state. Uploads use the same
//! explicit context to retain the initiating opening.
//!
//! The params ARE the wire types: [`SubscriptionSpec`] is what
//! `changes.subscribe` takes. Subscribe is an
//! upsert by `subscription_id`; re-sending an id with a new mode is how a
//! client changes cadence, and the bus keeps what that subscription already
//! held.
//!
//! One exception to "one shape": subscribe takes its kinds as words
//! ([`KindNames`]) and reads them itself, so a kind this bridge does not know
//! is refused by name — every one of them, in `details.kinds` — and a client
//! can drop exactly those and ask again (announced as `changes.refusedKinds`).

use super::Handler;
use crate::api::ApiError;
use crate::app::{AppState, WatchAnswer};
use crate::changes::{
    is_project_source_entity, Kind, KindNames, Scope, SubscriptionSpec, WatchState,
};
use crate::execution::CallContext as SessionSender;
use crate::{v1_context_method, v1_methods};
use serde::{Deserialize, Serialize};

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_context_method!(
            "changes.subscribe",
            changes_subscribe,
            SubscriptionSpec<KindNames>,
            Subscribed
        ),
        v1_context_method!(
            "changes.unsubscribe",
            changes_unsubscribe,
            UnsubscribeParams,
            Unsubscribed
        ),
    ]
}

// ---------------------------------------------------------------- params ---

#[derive(Debug, Deserialize, Serialize)]
pub struct UnsubscribeParams {
    pub subscription_id: String,
}

// --------------------------------------------------------------- results ---

#[derive(Debug, Deserialize, Serialize)]
pub struct Subscribed {
    pub subscription_id: String,
    /// `polled` means a worktree in scope could not get a filesystem watcher
    /// and its `git`/`files` kinds come from the TTL refresh instead.
    pub watch: WatchState,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Unsubscribed {
    pub ok: bool,
}

// -------------------------------------------------------------- handlers ---

/// Upsert one subscription for the calling session.
fn changes_subscribe(
    app: &mut AppState,
    session: &SessionSender,
    params: SubscriptionSpec<KindNames>,
) -> Result<Subscribed, ApiError> {
    if !session.events_are_live() {
        return Err(ApiError::unavailable("application event binding has ended"));
    }
    let params = known_kinds(params)?;
    check(&params)?;
    app.check_source_subscription(&params.scope, &params.kinds)
        .map_err(ApiError::classify)?;
    let subscription_id = params.id.clone();
    let outcome = app.changes().try_subscribe(session, params.clone())?;
    // The watchers are reconciled with the lock released, and the reply's
    // `watch` is read after that — this typed value is the placeholder the
    // drain replaces (see `AppState::defer_watch`).
    app.defer_watch(WatchAnswer::Subscribed(params));
    Ok(Subscribed {
        subscription_id,
        watch: outcome.watch,
    })
}

/// Drop one of this session's subscriptions. Idempotent: unsubscribing one
/// that is already gone is `{"ok": true}`, because the client's desired state
/// is what it asked for either way.
fn changes_unsubscribe(
    app: &mut AppState,
    session: &SessionSender,
    params: UnsubscribeParams,
) -> Result<Unsubscribed, ApiError> {
    if !session.events_are_live() {
        return Err(ApiError::unavailable("application event binding has ended"));
    }
    app.changes()
        .unsubscribe_one_scoped(session, &params.subscription_id);
    app.defer_watch(WatchAnswer::Unsubscribed);
    Ok(Unsubscribed { ok: true })
}

/// The spec with its kinds read, or `invalid_params` naming every kind this
/// bridge does not know: in a sentence, and in `details.kinds` for a client to
/// drop before it asks again. The rest of the request is refused with them —
/// nothing of it is subscribed.
fn known_kinds(params: SubscriptionSpec<KindNames>) -> Result<SubscriptionSpec, ApiError> {
    params.known().map_err(|unknown| {
        let (named, pronoun) = match unknown.split_last() {
            Some((last, rest)) if !rest.is_empty() => {
                (format!("{} and {last}", rest.join(", ")), "them")
            }
            _ => (unknown.join(""), "it"),
        };
        ApiError::InvalidParams {
            message: format!(
                "Build cannot subscribe to {named}: this bridge does not know {pronoun}."
            ),
            details: Some(serde_json::json!({ "kinds": unknown })),
        }
    })
}

/// What the bus will not enforce for us: a subscription with no id, no kinds,
/// or a board scope asking for more than the feed can move.
fn check(sub: &SubscriptionSpec) -> Result<(), ApiError> {
    if sub.id.trim().is_empty() {
        return Err(ApiError::invalid_params(
            "missing required param: subscription_id",
        ));
    }
    if sub.kinds.contains(Kind::CompactionChanged) && sub.scope != Scope::All {
        return Err(ApiError::invalid_params(
            "compaction.changed requires all scope",
        ));
    }
    if sub.kinds.is_empty() {
        return Err(ApiError::invalid_params(
            "changes.subscribe: name at least one kind — state, thread, git, files, or terminals",
        ));
    }
    if sub.scope == Scope::Board && sub.kinds.iter().any(|kind| kind != Kind::State) {
        return Err(ApiError::invalid_params(
            "changes.subscribe: a board scope carries state only",
        ));
    }
    if matches!(sub.scope, Scope::ProjectSource(_))
        && sub
            .kinds
            .iter()
            .any(|kind| !(kind.is_worktree() || kind == Kind::Terminals))
    {
        return Err(ApiError::invalid_params(
            "changes.subscribe: a project source scope carries git, files and terminals only",
        ));
    }
    if matches!(&sub.scope, Scope::Entity(id) if is_project_source_entity(id)) {
        return Err(ApiError::invalid_params(
            "changes.subscribe: a project source must be named by a project_source scope",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::{ChangeBus, KindSet, Mode, Priority};
    use serde_json::json;
    use std::time::Duration;

    fn round_trips(method: &str) {
        crate::api::v1::testing::fixture_round_trips(methods(), method);
    }

    #[test]
    fn the_changes_subscribe_fixture_round_trips() {
        round_trips("changes.subscribe");
    }

    #[test]
    fn the_changes_unsubscribe_fixture_round_trips() {
        round_trips("changes.unsubscribe");
    }

    fn focus() -> SubscriptionSpec {
        SubscriptionSpec {
            id: "s-focus".into(),
            scope: Scope::Entity("run-7".into()),
            kinds: KindSet::all(),
            mode: Mode::Realtime,
            priority: Priority::Foreground,
        }
    }

    /// The verbs are about the session, so a call with no session named is a
    /// refusal with a code and not a subscription nobody can be sent.
    #[test]
    fn a_call_with_no_session_is_unavailable() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = AppState::new_unrooted(temp.path(), "main", true, "unused.sock");
        let refused = crate::api::v1::dispatch(
            &mut app,
            "changes.unsubscribe",
            &json!({"subscription_id": "unused"}),
        )
        .unwrap()
        .unwrap_err();
        assert_eq!(refused.code(), "unavailable");
    }

    /// Subscribe, list, unsubscribe — all against the calling session, and
    /// the upsert keeps one subscription.
    #[test]
    fn the_family_serves_the_calling_sessions_subscriptions() {
        let bus = ChangeBus::new(Duration::from_millis(250));
        let (sender, _rx, _key) = SessionSender::observable("s-1");
        bus.subscribe(&sender, focus());
        bus.subscribe(
            &sender,
            SubscriptionSpec {
                mode: Mode::Batch(Duration::from_secs(30)),
                ..focus()
            },
        );
        assert_eq!(bus.list("s-1").len(), 1);
        bus.unsubscribe_one("s-1", "s-focus");
        assert!(bus.list("s-1").is_empty());
    }

    #[test]
    fn explicit_context_keeps_equal_subscription_ids_separate() {
        let temp = tempfile::tempdir().unwrap();
        let mut app = AppState::new_unrooted(temp.path(), "main", true, "unused.sock");
        let first = SessionSender::legacy(
            "context-first",
            crate::execution::session::Opening::new(),
            std::sync::Arc::new(|_| true),
        );
        let second = SessionSender::legacy(
            "context-second",
            crate::execution::session::Opening::new(),
            std::sync::Arc::new(|_| true),
        );
        let params = json!({
            "subscription_id": "shared-id",
            "scope": { "kind": "all" },
            "kinds": ["state"],
            "mode": "realtime"
        });
        for sender in [&first, &second] {
            crate::api::v1::dispatch_with_context(
                &mut app,
                "changes.subscribe",
                &params,
                Some(sender),
            )
            .unwrap()
            .unwrap();
        }
        let absent = crate::api::v1::dispatch(&mut app, "changes.subscribe", &params)
            .unwrap()
            .unwrap_err();
        assert_eq!(absent.code(), "unavailable");
        crate::api::v1::dispatch_with_context(
            &mut app,
            "changes.unsubscribe",
            &json!({"subscription_id": "shared-id"}),
            Some(&first),
        )
        .unwrap()
        .unwrap();
        assert!(app.changes().list(first.session_id()).is_empty());
        assert_eq!(app.changes().list(second.session_id()).len(), 1);
    }

    /// A board scope carries the feed's own state and nothing else; naming a
    /// worktree kind on it is a client bug worth a refusal.
    #[test]
    fn a_board_scope_may_not_ask_for_worktree_kinds() {
        let refused = check(&SubscriptionSpec {
            scope: Scope::Board,
            kinds: [Kind::Git].into_iter().collect(),
            ..focus()
        })
        .unwrap_err();
        assert_eq!(refused.code(), "invalid_params");
        assert!(refused.message().contains("state only"), "{refused:?}");

        check(&SubscriptionSpec {
            scope: Scope::Board,
            kinds: [Kind::State].into_iter().collect(),
            ..focus()
        })
        .expect("state on the board is the tier the feed subscribes with");
    }

    #[test]
    fn compaction_wire_subscription_requires_device_scope() {
        let subscription = SubscriptionSpec {
            scope: Scope::All,
            kinds: [Kind::CompactionChanged].into_iter().collect(),
            ..focus()
        };
        check(&subscription).expect("device invalidation subscription");
        let refused = check(&SubscriptionSpec {
            scope: Scope::Board,
            ..subscription
        })
        .unwrap_err();
        assert_eq!(refused.code(), "invalid_params");
        assert!(refused.message().contains("requires all scope"));
    }

    #[test]
    fn a_subscription_with_no_kinds_is_refused() {
        let refused = check(&SubscriptionSpec {
            kinds: KindSet::default(),
            ..focus()
        })
        .unwrap_err();
        assert_eq!(refused.code(), "invalid_params");
    }

    /// The kinds of a subscribe, as a client sends them.
    fn asking_for(kinds: &[&str]) -> SubscriptionSpec<KindNames> {
        SubscriptionSpec {
            id: "s-inbox".into(),
            scope: Scope::All,
            kinds: kinds.iter().copied().collect(),
            mode: Mode::Realtime,
            priority: Priority::Foreground,
        }
    }

    /// One kind this bridge does not know refuses the subscription, naming
    /// that kind in the sentence and in `details.kinds` for the client to drop.
    #[test]
    fn an_unknown_kind_is_refused_by_name() {
        let refused = known_kinds(asking_for(&["state", "reviews", "thread"])).unwrap_err();
        assert_eq!(refused.code(), "invalid_params");
        assert_eq!(
            refused.message(),
            "Build cannot subscribe to reviews: this bridge does not know it."
        );
        assert_eq!(refused.details(), Some(&json!({ "kinds": ["reviews"] })));
    }

    /// Several are all named at once, in the order asked and each once, so
    /// one retry without them is enough.
    #[test]
    fn every_unknown_kind_is_named_at_once() {
        let refused = known_kinds(asking_for(&[
            "reviews",
            "state",
            "sandwiches",
            "reviews",
            "pickles",
        ]))
        .unwrap_err();
        assert_eq!(refused.code(), "invalid_params");
        assert_eq!(
            refused.message(),
            "Build cannot subscribe to reviews, sandwiches and pickles: this bridge does not know them."
        );
        assert_eq!(
            refused.details(),
            Some(&json!({ "kinds": ["reviews", "sandwiches", "pickles"] }))
        );
    }

    /// Every kind it does know reads as that kind, deduplicated.
    #[test]
    fn known_kinds_read_as_the_set() {
        let read = known_kinds(asking_for(&["thread", "state", "thread", "tasks"])).unwrap();
        assert_eq!(
            read.kinds,
            [Kind::State, Kind::Thread, Kind::Tasks]
                .into_iter()
                .collect()
        );
        assert_eq!(read.id, "s-inbox");
    }

    /// The fixture's refusal is what the verb answers for the fixture's
    /// request, envelope and all — the shape the SPA's retry reads.
    #[test]
    fn the_changes_subscribe_refusal_fixture_round_trips() {
        let fixture = crate::api::v1::testing::fixture("changes.subscribe");
        let refusal = &fixture["refusal"];
        let params =
            crate::api::v1::parse_params::<SubscriptionSpec<KindNames>>(&refusal["params"])
                .expect("the refused request is well formed; only its kinds are not known");
        let refused = known_kinds(params).unwrap_err();
        let reply = refused.into_reply(json!("r1"));
        let mut expected = refusal["reply"].clone();
        expected["id"] = json!("r1");
        assert_eq!(reply, expected);
    }

    /// A field this bridge predates is refused by name, as a kind it predates
    /// is: a client asks for what the greeting advertises.
    #[test]
    fn params_from_a_newer_client_are_refused_by_name() {
        let refused = crate::api::v1::parse_params::<SubscriptionSpec>(&json!({
            "subscription_id": "s-focus",
            "scope": { "kind": "entity", "id": "run-7" },
            "kinds": ["state"],
            "mode": "realtime",
            "settle_ms": 40,
        }))
        .expect_err("a field this bridge predates is not quietly dropped");
        assert_eq!(refused.code(), "invalid_params");
        assert_eq!(refused.message(), "unknown param: settle_ms");
    }
}
