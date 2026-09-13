//! The v1 wire, one module per verb family (wire spec Part 2, step 2.2).
//!
//! Each family holds typed request and response structs and a `methods()`
//! table naming the verbs it serves. [`dispatch`] parses the params into the
//! verb's typed struct, runs the handler, and serialises the typed result;
//! `app/rpc.rs::route` asks here first and falls through to its legacy arms
//! for anything unregistered. Requests derive `Deserialize` without
//! `deny_unknown_fields` (a newer client may send what this bridge predates);
//! results derive `Serialize` with `skip_serializing_if` on optionals, so an
//! absent field and a `null` one read the same.
//!
//! A handler signature never mentions `serde_json::Value` — the test at the
//! bottom scans the family files for one. An implementation change that
//! alters a shape is a change to a type here, which is a change to a fixture
//! under `fixtures/api/v1/`, which is a version bump.
//!
//! NOT here, by design: `session.hello`, `ping`, `bridge.stats`, `term.*`,
//! `rtc.*`, `agent.attach` and `agent.start`. Each needs the caller's own
//! `SessionSender` (somewhere to push to) or the shared `Arc` (a producer or
//! pump to spawn), which `dispatch` deliberately has no access to; they stay
//! on the legacy route in `app/rpc.rs::dispatch_frame`.
//!
//! Families: [`board`] (`board.list`, `archive.list`, `archived.list`,
//! `project.*`, `capture.*`, `settings.*`, `models.list`), [`thread`]
//! (`thread.*`, `agent.add/choose/remove/list`), [`git`] (`git.*`, `fs.*`,
//! and the diff reads), [`lifecycle`] (`issue.*`, `plan.*`, `run.*`,
//! `branch.*`, `worktree.create/finish`, `entity.*`, `triage.override`).

pub mod board;
pub mod git;
pub mod lifecycle;
pub mod thread;

use crate::api::ApiError;
use crate::app::AppState;
use serde::de::DeserializeOwned;
use serde::Serialize;
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
/// names it, and [`Answer`] carries the placeholder.
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
    ) -> Handler {
        Handler {
            call,
            parse_params,
            round_trip_result,
        }
    }
}

/// What a v1 handler gives back: the wire value the existing implementation
/// produced, with `R` naming the shape that value takes.
///
/// The implementations under `app/` predate the facade and answer in
/// [`Value`]; rewriting them to build `R` would rewrite the git, so `R` is
/// carried as a type parameter instead. It is not decoration: the contract
/// test holds every fixture to it, and in test builds [`answer`] checks the
/// value the implementation actually produced against it too.
///
/// A [`Value::Null`] is the deferral placeholder `AppState::deferred_work`
/// documents — the handler resolved the request under the lock and queued the
/// work; the drain replaces this with the real result, so there is nothing
/// here to check.
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

/// The params of a v1 verb, as the pre-facade implementation still reads
/// them. Typed params are the contract; the implementation underneath takes a
/// [`Value`], so a handler hands it one built back from the typed struct —
/// which also drops any field this bridge does not know, so an implementation
/// can never read past its own contract.
pub trait WireParams: Serialize {
    fn wire(&self) -> Value {
        serde_json::to_value(self).unwrap_or(Value::Null)
    }
}

impl<T: Serialize> WireParams for T {}

/// Parse `params` into `P`, naming a missing field the way every bridge verb
/// always has (`missing required param: <field>`).
pub fn parse_params<P: DeserializeOwned>(params: &Value) -> Result<P, ApiError> {
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
pub fn parse_as<P: DeserializeOwned>(params: &Value) -> Result<(), String> {
    parse_params::<P>(params)
        .map(|_| ())
        .map_err(|error| error.message().to_string())
}

#[doc(hidden)]
pub fn round_trip_as<R: DeserializeOwned + Serialize>(result: &Value) -> Result<Value, String> {
    let parsed: R = serde_json::from_value(result.clone()).map_err(|e| e.to_string())?;
    serde_json::to_value(parsed).map_err(|e| e.to_string())
}

/// Run one typed handler over untyped params: parse, call, serialise.
#[doc(hidden)]
pub fn call_typed<P: DeserializeOwned, R: Serialize>(
    app: &mut AppState,
    params: &Value,
    handler: fn(&mut AppState, P) -> Result<R, ApiError>,
) -> Result<Value, ApiError> {
    let parsed = parse_params::<P>(params)?;
    let result = handler(app, parsed)?;
    serde_json::to_value(result).map_err(|error| ApiError::internal(error.to_string()))
}

/// Every family's table, in one place.
fn families() -> [&'static [(&'static str, Handler)]; 4] {
    [
        board::methods(),
        thread::methods(),
        git::methods(),
        lifecycle::methods(),
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
    Some((handler.call)(app, params))
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
        #[derive(Debug, serde::Deserialize)]
        struct Needs {
            #[allow(dead_code)]
            paths: Vec<String>,
        }
        let refused = parse_params::<Needs>(&serde_json::json!({})).unwrap_err();
        assert_eq!(refused.message(), "missing required param: paths");
        assert_eq!(refused.code(), "invalid_params");
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
