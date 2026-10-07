//! The changes family: `changes.subscribe` and `changes.unsubscribe` (wire
//! spec Part 1, step 1.1).
//!
//! Two verbs over [`crate::changes::ChangeBus`], whose subject is the session
//! itself: a subscription is an address to push to, so
//! every verb here needs the caller's own [`SessionSender`]. [`dispatch`]
//! deliberately carries only `AppState`, so the frame handler names the
//! caller for the duration of the call with [`with_session`] and the handlers
//! read it back. Called with no session named — the synchronous test entry
//! point, or an internal probe — the verbs refuse `unavailable` rather than
//! guessing at a subscriber.
//! Uploads use the same caller context to bind their state to the initiating
//! encrypted session opening.
//!
//! [`dispatch`]: crate::api::v1::dispatch
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
use crate::carrier::SessionSender;
use crate::changes::{Kind, KindNames, Scope, SubscriptionSpec, WatchState};
use crate::{v1_method, v1_methods};
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

/// The verbs this family serves.
pub fn methods() -> &'static [(&'static str, Handler)] {
    v1_methods![
        v1_method!(
            "changes.subscribe",
            changes_subscribe,
            SubscriptionSpec<KindNames>,
            Subscribed
        ),
        v1_method!(
            "changes.unsubscribe",
            changes_unsubscribe,
            UnsubscribeParams,
            Unsubscribed
        ),
    ]
}

// ------------------------------------------------------- the caller ---

thread_local! {
    /// Who is asking, for the length of one frame. A thread-local rather than
    /// a param because the facade's dispatch signature is one shape for every
    /// verb, and subscriptions and uploads both need the session.
    static CALLER: RefCell<Option<SessionSender>> = const { RefCell::new(None) };
}

/// Name the session this frame came from while `run` executes. Nested calls
/// restore the previous caller, and a panic inside `run` restores it too.
pub fn with_session<T>(session: &SessionSender, run: impl FnOnce() -> T) -> T {
    struct Restore(Option<SessionSender>);
    impl Drop for Restore {
        fn drop(&mut self) {
            CALLER.with(|caller| *caller.borrow_mut() = self.0.take());
        }
    }
    let _restore = Restore(CALLER.with(|caller| caller.borrow_mut().replace(session.clone())));
    run()
}

pub(crate) fn caller() -> Result<SessionSender, ApiError> {
    CALLER
        .with(|caller| caller.borrow().clone())
        .ok_or_else(|| {
            ApiError::unavailable(
                "no session on this call — this method needs the initiating client",
            )
        })
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
    params: SubscriptionSpec<KindNames>,
) -> Result<Subscribed, ApiError> {
    let params = known_kinds(params)?;
    let session = caller()?;
    check(&params)?;
    let subscription_id = params.id.clone();
    let outcome = app.changes().subscribe(&session, params.clone());
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
    params: UnsubscribeParams,
) -> Result<Unsubscribed, ApiError> {
    let session = caller()?;
    app.changes()
        .unsubscribe_one(session.session_id(), &params.subscription_id);
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
        let refused = caller().err().expect("no session, no subscription");
        assert_eq!(refused.code(), "unavailable");
    }

    /// Subscribe, list, unsubscribe — all against the calling session, and
    /// the upsert keeps one subscription.
    #[test]
    fn the_family_serves_the_calling_sessions_subscriptions() {
        let bus = ChangeBus::new(Duration::from_millis(250));
        let (sender, _rx, _key) = SessionSender::observable("s-1");
        with_session(&sender, || {
            let session = caller().unwrap();
            assert_eq!(session.session_id(), "s-1");
            bus.subscribe(&session, focus());
            bus.subscribe(
                &session,
                SubscriptionSpec {
                    mode: Mode::Batch(Duration::from_secs(30)),
                    ..focus()
                },
            );
            assert_eq!(bus.list("s-1").len(), 1);
            bus.unsubscribe_one("s-1", "s-focus");
            assert!(bus.list("s-1").is_empty());
        });
        assert!(caller().is_err(), "the caller is only named for the frame");
    }

    /// A board scope carries the feed's own state and nothing else; naming a
    /// worktree kind on it is a client bug worth a refusal.
    #[test]
    fn a_board_scope_may_not_ask_for_worktree_kinds() {
        let refused = check(&SubscriptionSpec {
            scope: Scope::Board,
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
