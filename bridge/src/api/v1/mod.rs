//! The v1 wire, one module per verb family (wire spec Part 2, step 2.2).
//!
//! Each family holds typed request and response structs and a `methods()`
//! table naming the verbs it serves. [`dispatch`] parses the params into the
//! verb's typed struct, runs the handler, and serialises the typed result;
//! `app/rpc.rs::route` asks here first and falls through to its legacy arms
//! for anything unregistered. A top-level param the verb's type does not
//! declare is refused (`invalid_params`, `unknown param: <name>`), never
//! dropped: a client sends a newer param only once the greeting announces
//! it. The check is [`parse_params`]'s, not `deny_unknown_fields`, which
//! cannot see through the `#[serde(flatten)]`ed scopes. Nested objects keep
//! their own rules. Results derive `Serialize` with `skip_serializing_if` on
//! optionals, so an absent field and a `null` one read the same.
//!
//! A handler signature never mentions `serde_json::Value` — the test at the
//! bottom scans the family files for one. An implementation change that
//! alters a shape is a change to a type here, which is a change to a fixture
//! under `fixtures/api/v1/` and, for an additive feature, its announced name.
//! Additive features share a release's minor version: do not bump for each
//! verb or field independently. Only a breaking removal or shape change
//! requires a new major. `session.hello.capabilities` lists each registered
//! verb by this table's exact name and names cross-verb wire features; the
//! contract test prevents a fixture verb from lacking a capability.
//!
//! NOT here, by design: `session.hello`, `ping`, `bridge.stats`, `term.*`,
//! `rtc.*`, `agent.attach`, `agent.start` and `agent.interrupt`. Each needs
//! the caller's own `SessionSender` (somewhere to push to) or the shared
//! `Arc` (a producer or pump to spawn), which `dispatch` deliberately has no
//! access to; they stay on the legacy route in `app/rpc.rs::dispatch_frame`.
//!
//! The retired planning workflow keeps only its reads (`task.get`,
//! `task.stages`, `task.doc`, `task.stage_doc`, `task.stage_diff`), for the
//! Tasks stored before it was retired; every verb that mutated it was cut
//! (#207) and answers `unknown_method` like any verb this bridge never had.
//!
//! Families: [`board`] (`board.list`, `archived.list`, `project.*`,
//! `capture.*`, `settings.*`, `models.list`), [`thread`] (`thread.*`,
//! `agent.add/choose/remove`), [`changes`] (`changes.subscribe/unsubscribe`),
//! [`git`] (`git.*`, `fs.*`, and the diff reads), [`github`]
//! (`github.repos`), [`lifecycle`] (the `task.*` reads, `run.*`,
//! `branch.dispatch/finish`, `entity.*`),
//! [`workspace`] (`workspace.*`), [`updates`] (`bridge.update_status`,
//! `bridge.check_update`, `bridge.install_update`), [`push`] (`push.registerKey`,
//! `push.revokeKey`, the notification keys sealed push content goes to),
//! [`tasks`] (`tasks.*`, the per-project
//! tracker — NOT `lifecycle`'s singular `task.*`, which is the retired plan
//! flow).

pub mod board;
pub mod changes;
pub mod git;
pub mod github;
pub mod lifecycle;
pub mod push;
pub mod tasks;
pub mod thread;
pub mod updates;
pub mod workspace;

use crate::api::ApiError;
use crate::app::AppState;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::sync::OnceLock;

/// One registered verb: how to run it, and how the contract test holds its
/// fixture to its types.
pub struct Handler {
    call: fn(&mut AppState, &Value) -> Result<Value, ApiError>,
    parse_params: fn(&Value) -> Result<(), String>,
    round_trip_result: fn(&Value) -> Result<Value, String>,
    /// The verb's declared result type, as a check a value must pass. Handed
    /// to the drain by [`dispatch`] for a verb that deferred its work, whose
    /// real answer this facade never sees.
    check_result: crate::app::DeferredResultCheck,
}

impl Handler {
    /// Parse `params` into the verb's typed params, keeping nothing.
    pub fn parse_params(&self, params: &Value) -> Result<(), String> {
        (self.parse_params)(params)
    }

    /// Parse `result` into the verb's typed result and serialise it back —
    /// what the contract test compares to the fixture.
    pub fn round_trip_result(&self, result: &Value) -> Result<Value, String> {
        (self.round_trip_result)(result)
    }
}

/// A family's whole table. The entries cannot be const-promoted (each carries
/// a coerced function pointer), so the table is built once on first use and
/// leaked into `'static` by the [`OnceLock`] holding it.
#[macro_export]
macro_rules! v1_methods {
    ($($method:expr),* $(,)?) => {{
        static TABLE: ::std::sync::OnceLock<
            ::std::vec::Vec<(&'static str, $crate::api::v1::Handler)>,
        > = ::std::sync::OnceLock::new();
        TABLE.get_or_init(|| ::std::vec![$($method),*]).as_slice()
    }};
}

/// Build a [`Handler`] from a typed handler function, naming the params it
/// takes and the result its fixture holds. The result named is the wire shape
/// — for a verb that answers through the deferred drain, the handler still
/// names it, [`Answer`] carries the placeholder, and the drain holds the real
/// value to the same type ([`Handler::check_result`]).
#[macro_export]
macro_rules! v1_method {
    ($name:literal, $handler:path, $params:ty, $result:ty) => {
        (
            $name,
            $crate::api::v1::Handler::new(
                |app: &mut $crate::app::AppState, params: &::serde_json::Value| {
                    $crate::api::v1::call_typed(app, params, $handler)
                },
                $crate::api::v1::parse_as::<$params>,
                $crate::api::v1::round_trip_as::<$result>,
                $crate::api::v1::check_as::<$result>,
            ),
        )
    };
}

impl Handler {
    #[doc(hidden)]
    pub const fn new(
        call: fn(&mut AppState, &Value) -> Result<Value, ApiError>,
        parse_params: fn(&Value) -> Result<(), String>,
        round_trip_result: fn(&Value) -> Result<Value, String>,
        check_result: crate::app::DeferredResultCheck,
    ) -> Handler {
        Handler {
            call,
            parse_params,
            round_trip_result,
            check_result,
        }
    }
}

/// What a v1 handler gives back: the wire value the existing implementation
/// produced, with `R` naming the shape that value takes.
///
/// The implementations under `app/` predate the facade and answer in
/// [`Value`]; rewriting them to build `R` would rewrite the git, so `R` is
/// carried as a type parameter instead. It is not decoration, and nothing a
/// verb answers escapes it:
///
/// - the contract test holds every fixture to it;
/// - in test builds [`answer`] checks the value the implementation produced
///   against it, here, where the handler answered directly;
/// - a [`Value::Null`] is the deferral placeholder `AppState::deferred_work`
///   documents — the handler resolved the request under the lock and queued
///   the work, and the real value only exists after the mutex is released.
///   There is nothing to check HERE, so [`dispatch`] hands the check
///   ([`Handler::check_result`]) to the drain with the job, and
///   `AppState::apply_deferred` runs it on the value it is about to publish.
///   That is every `git.*` verb and every diff read, in release builds as
///   well as test ones; a mismatch is `internal`, naming the method and what
///   serde refused.
pub struct Answer<R> {
    value: Value,
    shape: PhantomData<R>,
}

impl<R> Serialize for Answer<R> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.value.serialize(serializer)
    }
}

/// Wrap what an existing implementation answered, naming its shape. A bare
/// `Err(String)` is given a code here — the facade is where the spec says the
/// naming happens.
pub fn answer<R: DeserializeOwned>(outcome: Result<Value, String>) -> Result<Answer<R>, ApiError> {
    let value = outcome.map_err(ApiError::classify)?;
    #[cfg(test)]
    if !value.is_null() {
        if let Err(error) = serde_json::from_value::<R>(value.clone()) {
            panic!("the answer does not match the type api/v1 declares for it: {error}\n{value}");
        }
    }
    Ok(Answer {
        value,
        shape: PhantomData,
    })
}

/// Read a deferring verb's lock-held acknowledgement as the placeholder it is.
///
/// Most deferring verbs answer `Value::Null` under the lock and let the drain
/// produce the real value; the workspace finish answers
/// `{"workspace_id": ..., "pending": true}` instead. Neither is what a client
/// sees — `AppState::apply_deferred` replaces both — so a handler over that
/// second spelling maps it here, and [`answer`] gets the same "nothing to
/// check yet" it gets from every other deferring verb. The declared result
/// type is unchanged, and [`Handler::check_result`] still holds the value the
/// drain publishes to it.
pub fn deferral_placeholder(value: Value) -> Value {
    match value.get("pending") {
        Some(Value::Bool(true)) => Value::Null,
        _ => value,
    }
}

/// The params of a v1 verb, as the pre-facade implementation still reads
/// them. Typed params are the contract; the implementation underneath takes a
/// [`Value`], so a handler hands it one built back from the typed struct, and
/// an implementation can never read past its own contract. A field the
/// contract does not name never gets this far: [`parse_params`] refuses it.
pub trait WireParams: Serialize {
    fn wire(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

impl<T: Serialize> WireParams for T {}

/// A verb that names nothing but itself — a read of what the account holds
/// (`board.list`, `project.list`, `settings.get`, ...). One type for every
/// family, so "takes nothing" is spelled once.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct NoParams {}

/// Parse `params` into `P`, naming a missing field the way every bridge verb
/// always has (`missing required param: <field>`), and refusing a field `P`
/// does not declare (`unknown param: <field>`) rather than dropping it.
pub fn parse_params<P: DeserializeOwned + Serialize>(params: &Value) -> Result<P, ApiError> {
    let parsed = parse_declared::<P>(params)?;
    let unknown = undeclared_params::<P>(params, &parsed);
    if unknown.is_empty() {
        Ok(parsed)
    } else {
        Err(unknown_params(unknown))
    }
}

/// A value no declared field holds, standing in for a param's own.
const PROBE: &str = "\u{0}build: is this param declared?";

/// The top-level params `P` swallows: those whose value changes nothing.
///
/// `deny_unknown_fields` cannot say this for the families' types — it is not
/// supported with `#[serde(flatten)]`, which every scoped verb uses. So the
/// question is put to `P` itself. A field that survives the round trip is
/// declared; one that did not is swapped for [`PROBE`], and a declared field
/// either refuses that or carries it into what `P` holds. A field `P` holds
/// nothing of whatever its value is was never read. Only fields the round
/// trip dropped are probed, so a request of declared, non-default fields
/// parses once, as it always did.
fn undeclared_params<P: DeserializeOwned + Serialize>(params: &Value, parsed: &P) -> Vec<String> {
    let Value::Object(given) = params else {
        return Vec::new();
    };
    let Ok(held) = serde_json::to_value(parsed) else {
        return Vec::new();
    };
    given
        .keys()
        .filter(|name| held.get(name.as_str()).is_none())
        .filter(|name| {
            let mut probed = given.clone();
            probed.insert((*name).clone(), Value::String(PROBE.into()));
            serde_json::from_value::<P>(Value::Object(probed))
                .ok()
                .and_then(|holds| serde_json::to_value(holds).ok())
                .is_some_and(|holds| holds == held)
        })
        .cloned()
        .collect()
}

/// The refusal for fields a verb does not declare, every one named; sorted,
/// because a JSON object's keys are.
fn unknown_params(names: Vec<String>) -> ApiError {
    let message = match names.as_slice() {
        [one] => format!("unknown param: {one}"),
        many => format!("unknown params: {}", many.join(", ")),
    };
    ApiError::InvalidParams {
        message,
        details: Some(serde_json::json!({ "params": names })),
    }
}

fn parse_declared<P: DeserializeOwned>(params: &Value) -> Result<P, ApiError> {
    serde_json::from_value(params.clone()).map_err(|error| {
        let message = error.to_string();
        let message = match message.strip_prefix("missing field `") {
            Some(rest) => format!(
                "missing required param: {}",
                rest.split('`').next().unwrap_or_default()
            ),
            None => message,
        };
        ApiError::invalid_params(message)
    })
}

#[doc(hidden)]
pub fn parse_as<P: DeserializeOwned + Serialize>(params: &Value) -> Result<(), String> {
    parse_params::<P>(params)
        .map(|_| ())
        .map_err(|error| error.message().to_string())
}

/// The verb's declared result type as a check: does this value parse as `R`?
/// What the drain holds a deferred reply to.
#[doc(hidden)]
pub fn check_as<R: DeserializeOwned>(result: &Value) -> Result<(), String> {
    serde_json::from_value::<R>(result.clone())
        .map(|_| ())
        .map_err(|error| error.to_string())
}

#[doc(hidden)]
pub fn round_trip_as<R: DeserializeOwned + Serialize>(result: &Value) -> Result<Value, String> {
    let parsed: R = serde_json::from_value(result.clone()).map_err(|e| e.to_string())?;
    serde_json::to_value(parsed).map_err(|e| e.to_string())
}

/// Run one typed handler over untyped params: parse, call, serialise.
#[doc(hidden)]
pub fn call_typed<P: DeserializeOwned + Serialize, R: Serialize>(
    app: &mut AppState,
    params: &Value,
    handler: fn(&mut AppState, P) -> Result<R, ApiError>,
) -> Result<Value, ApiError> {
    let parsed = parse_params::<P>(params)?;
    let result = handler(app, parsed)?;
    serde_json::to_value(result).map_err(|error| ApiError::internal(error.to_string()))
}

/// Every family's table, in one place.
fn families() -> [&'static [(&'static str, Handler)]; 10] {
    [
        board::methods(),
        changes::methods(),
        thread::methods(),
        git::methods(),
        github::methods(),
        tasks::methods(),
        lifecycle::methods(),
        push::methods(),
        updates::methods(),
        workspace::methods(),
    ]
}

/// Every verb v1 serves, by name.
pub fn methods() -> &'static [(&'static str, Handler)] {
    static ALL: OnceLock<Vec<(&'static str, Handler)>> = OnceLock::new();
    ALL.get_or_init(|| {
        families()
            .into_iter()
            .flatten()
            .map(|(name, handler)| {
                (
                    *name,
                    Handler::new(
                        handler.call,
                        handler.parse_params,
                        handler.round_trip_result,
                        handler.check_result,
                    ),
                )
            })
            .collect()
    })
}

fn registry() -> &'static BTreeMap<&'static str, &'static Handler> {
    static REGISTRY: OnceLock<BTreeMap<&'static str, &'static Handler>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        methods()
            .iter()
            .map(|(name, handler)| (*name, handler))
            .collect()
    })
}

/// Serve `method` if v1 registers it: `None` means the legacy route answers.
pub fn dispatch(
    app: &mut AppState,
    method: &str,
    params: &Value,
) -> Option<Result<Value, ApiError>> {
    let handler = registry().get(method)?;
    let answered = (handler.call)(app, params);
    // A verb that handed its git to the drain answered `Value::Null` here, so
    // the check in `answer` had nothing to look at. Send the declared type
    // along with the work instead: it is checked when the real value lands.
    app.expect_deferred_result(handler.check_result);
    Some(answered)
}

/// What every family's unit tests hold their fixtures to. The contract test
/// in `tests/api_contract.rs` sweeps every family at once; a family's own
/// tests call these per verb, so a shape that drifts names the verb that
/// drifted.
#[cfg(test)]
pub(crate) mod testing {
    use super::Handler;
    use serde_json::Value;
    use std::path::Path;

    /// `fixtures/api/v1/<method>.json`, parsed.
    pub(crate) fn fixture(method: &str) -> Value {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../fixtures/api/v1")
            .join(format!("{method}.json"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
    }

    /// The fixture for `method`, held to the types `family` registers for it:
    /// its params parse, and its result comes back out of the typed result
    /// exactly as it went in.
    pub(crate) fn fixture_round_trips(family: &[(&str, Handler)], method: &str) {
        let fixture = fixture(method);
        assert_eq!(fixture["method"], method, "the fixture names its method");
        let (_, handler) = family
            .iter()
            .find(|(name, _)| *name == method)
            .unwrap_or_else(|| panic!("{method} is not registered"));
        handler
            .parse_params(&fixture["params"])
            .unwrap_or_else(|error| panic!("{method}: params do not parse: {error}"));
        let round_tripped = handler
            .round_trip_result(&fixture["result"])
            .unwrap_or_else(|error| panic!("{method}: result does not parse: {error}"));
        assert_eq!(round_tripped, fixture["result"], "{method}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn no_verb_is_registered_by_two_families() {
        let mut seen = std::collections::BTreeSet::new();
        for (name, _) in methods() {
            assert!(seen.insert(*name), "{name} is registered twice");
        }
    }

    #[test]
    fn a_missing_required_param_reads_as_the_bridge_always_spelled_it() {
        #[derive(Debug, serde::Deserialize, serde::Serialize)]
        struct Needs {
            #[allow(dead_code)]
            paths: Vec<String>,
        }
        let refused = parse_params::<Needs>(&serde_json::json!({})).unwrap_err();
        assert_eq!(refused.message(), "missing required param: paths");
        assert_eq!(refused.code(), "invalid_params");
    }

    /// The shapes a verb's params take in the families: a flattened scope, an
    /// optional left out of the wire when absent, an alias, a required field.
    #[derive(Debug, Deserialize, Serialize)]
    struct Scope {
        project_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        worktree_id: Option<String>,
    }

    #[derive(Debug, Deserialize, Serialize)]
    struct Diff {
        #[serde(flatten)]
        scope: Scope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        patch: Option<bool>,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        staged: bool,
        #[serde(default, alias = "plan_id", skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        assignee: Option<Value>,
    }

    /// A field the verb does not declare is refused by name, as an unknown
    /// push kind refuses its whole subscription: answering `ok` to a request
    /// whose half was never read tells the client the half happened.
    #[test]
    fn a_param_the_verb_does_not_declare_is_refused_by_name() {
        let refused = parse_params::<Diff>(&serde_json::json!({
            "project_id": "proj-1",
            "pacth": false,
        }))
        .expect_err("a misspelt patch is not quietly the whole patch");
        assert_eq!(refused.code(), "invalid_params");
        assert_eq!(refused.message(), "unknown param: pacth");
        assert_eq!(
            refused.details(),
            Some(&serde_json::json!({ "params": ["pacth"] }))
        );
    }

    #[test]
    fn every_undeclared_param_is_named_at_once() {
        let refused = parse_params::<Diff>(&serde_json::json!({
            "project_id": "proj-1",
            "zeta": 1,
            "alpha": null,
        }))
        .unwrap_err();
        assert_eq!(refused.message(), "unknown params: alpha, zeta");
        assert_eq!(
            refused.details(),
            Some(&serde_json::json!({ "params": ["alpha", "zeta"] }))
        );
    }

    /// A declared field the typed struct drops on the way back out — a `null`
    /// optional, a `false` flag, an alias, a field of a flattened scope — is
    /// still declared, and is not mistaken for an unknown one.
    #[test]
    fn a_declared_param_at_its_default_is_not_unknown() {
        let parsed = parse_params::<Diff>(&serde_json::json!({
            "project_id": "proj-1",
            "worktree_id": null,
            "patch": null,
            "staged": false,
            "plan_id": "run-7",
            "assignee": null,
        }))
        .expect("every one of these is declared");
        assert_eq!(parsed.run_id.as_deref(), Some("run-7"));
    }

    #[test]
    fn a_verb_that_takes_nothing_refuses_whatever_it_is_given() {
        parse_params::<NoParams>(&serde_json::json!({})).expect("nothing is nothing");
        let refused =
            parse_params::<NoParams>(&serde_json::json!({ "project_id": "proj-1" })).unwrap_err();
        assert_eq!(refused.message(), "unknown param: project_id");
    }

    /// The signature text of every `fn` in a family file: from `fn` to the
    /// opening brace (or the `;` of a declaration), whitespace collapsed.
    fn signatures(source: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = source;
        while let Some(at) = rest.find("fn ") {
            let preceded_by_word =
                at > 0 && rest.as_bytes()[at - 1] != b' ' && rest.as_bytes()[at - 1] != b'\n';
            let candidate = &rest[at..];
            if !preceded_by_word {
                let end = candidate.find(['{', ';']).unwrap_or(candidate.len());
                found.push(
                    candidate[..end]
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            rest = &candidate[3..];
        }
        found
    }

    #[test]
    fn no_family_handler_signature_mentions_value() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/api/v1");
        let mut checked = 0;
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.file_name().is_some_and(|name| name == "mod.rs")
                || path.extension().is_none_or(|ext| ext != "rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            for signature in signatures(&source) {
                checked += 1;
                assert!(
                    !signature.contains("Value"),
                    "{}: `{signature}` takes or returns serde_json::Value; type it",
                    path.display()
                );
            }
        }
        assert!(checked > 0, "the scan found no signatures at all");
    }

    #[test]
    fn the_scan_would_catch_a_value_returning_handler() {
        let offending = "pub fn bad(app: &mut AppState, params: &Value)\n    -> Result<Value, ApiError> {\n    todo!()\n}\nfn ok(x: u8) -> u8 { x }\n";
        let found = signatures(offending);
        assert_eq!(found.len(), 2, "{found:?}");
        assert!(found[0].contains("Value"));
        assert!(!found[1].contains("Value"));
    }
}
