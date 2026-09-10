//! Learned review defaults: `.build/review-rules.json`, in a project's primary
//! checkout.
//!
//! Every time a reviewer disagrees with a triage level the disagreement is
//! recorded twice. Once on the run, against the hunk it was about — that is
//! what the review surface renders. And once here, generalized to the pattern
//! the hunk's file shares with its neighbours and counted, because "this file
//! was collapsed and should not have been" is worth very little on its own and
//! quite a lot the fourth time it happens in the same directory.
//!
//! Nothing consumes the counts automatically yet: this is the seed the
//! learned-defaults layer will grow from. Until it exists the file is read by
//! the only reader that can act on it today — the TRIAGE prompt, which is told
//! to open it when it is there and respect what the reviewer has already said.
//!
//! The file lives in the repository, so a human may hand-edit it. Every merge
//! here is done on the parsed document rather than on a typed struct, so a
//! field this module never writes (`path_glob`, a comment, a rule shape from a
//! later version) survives a bridge-written update untouched.

use serde_json::{json, Map, Value};

use crate::run::OverrideDirection;

/// Where the file sits, relative to the primary checkout.
pub const REVIEW_RULES_PATH: &str = ".build/review-rules.json";

/// The document version this module writes. Present so a later shape can be
/// told apart from this one without guessing.
const DOCUMENT_VERSION: u64 = 1;

/// The pattern one file path stands for.
///
/// A reviewer disagreeing about `spa/src/core/diff.js` is rarely making a point
/// about that one file — they are saying something about the JavaScript in that
/// directory. So the rule is keyed on the directory and the extension, which is
/// the smallest generalization that can accumulate a count at all.
///
/// A file with no extension generalizes to nothing useful (`docs/*` would sweep
/// in every sibling regardless of kind), so it stands for itself.
pub fn pattern_for_path(path: &str) -> String {
    let (directory, file) = match path.rsplit_once('/') {
        Some((directory, file)) => (Some(directory), file),
        None => (None, path),
    };
    // A leading dot names a hidden file, not an extension: `.gitignore` has no
    // extension, `.eslintrc.json` has `json`.
    let extension = file
        .rsplit_once('.')
        .filter(|(stem, extension)| !stem.is_empty() && !extension.is_empty())
        .map(|(_, extension)| extension);
    match (directory, extension) {
        (Some(directory), Some(extension)) => format!("{directory}/*.{extension}"),
        (None, Some(extension)) => format!("*.{extension}"),
        (_, None) => path.to_string(),
    }
}

/// Fold one more disagreement into `document`, returning the updated document
/// and the pattern's new count.
///
/// A rule is identified by its pattern and its direction: the same pattern
/// pulled both ways is two rules, and reading their counts side by side is the
/// only way the disagreement is visible at all. Repeats merge into one rule
/// with a higher count rather than a longer list.
///
/// `Value::Null` stands for "no file yet" — the caller does not have to decide
/// what an absent file means.
pub fn merge_override(
    document: Value,
    pattern: &str,
    direction: OverrideDirection,
    now: &str,
) -> Result<(Value, u64), String> {
    let mut document = match document {
        Value::Null => Map::new(),
        Value::Object(object) => object,
        _ => return Err(format!("{REVIEW_RULES_PATH} is not a JSON object")),
    };
    document.insert("version".to_string(), json!(DOCUMENT_VERSION));
    let mut rules = match document.remove("rules") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(rules)) => rules,
        Some(_) => return Err(format!("{REVIEW_RULES_PATH}: rules is not an array")),
    };
    let existing = rules.iter_mut().find(|rule| {
        rule.get("pattern").and_then(Value::as_str) == Some(pattern)
            && rule.get("direction").and_then(Value::as_str) == Some(direction.as_str())
    });
    let count = match existing {
        Some(Value::Object(rule)) => {
            let count = rule.get("count").and_then(Value::as_u64).unwrap_or(0) + 1;
            rule.insert("count".to_string(), json!(count));
            rule.insert("updated_at".to_string(), json!(now));
            count
        }
        // A rule that matched but is not an object cannot be matched — the
        // finder only reads fields off objects.
        Some(_) => unreachable!("only objects carry a pattern"),
        None => {
            rules.push(json!({
                "pattern": pattern,
                "direction": direction.as_str(),
                "count": 1,
                "updated_at": now,
            }));
            1
        }
    };
    document.insert("rules".to_string(), Value::Array(rules));
    Ok((Value::Object(document), count))
}

/// Read the checkout's rules document, or `Value::Null` when there is none.
///
/// A file that exists and does not parse is an error, not an empty document:
/// overwriting what a human wrote because the bridge could not read it would
/// lose the signal this file exists to keep.
pub fn read(checkout: &std::path::Path) -> Result<Value, String> {
    let path = checkout.join(REVIEW_RULES_PATH);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Value::Null),
        Err(error) => return Err(format!("reading {}: {error}", path.display())),
    };
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&text).map_err(|error| format!("parsing {}: {error}", path.display()))
}

/// Replace the checkout's rules document atomically.
pub fn write(checkout: &std::path::Path, document: &Value) -> Result<(), String> {
    let path = checkout.join(REVIEW_RULES_PATH);
    let json = serde_json::to_string_pretty(document)
        .map_err(|error| format!("serializing {REVIEW_RULES_PATH}: {error}"))?;
    crate::store::write_file_atomically(&path, &format!("{json}\n"))
        .map_err(|error| format!("writing {}: {error}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A disagreement about one file is a claim about the files beside it —
    /// which is the only reason a count can ever reach two.
    #[test]
    fn a_path_stands_for_the_pattern_its_neighbours_share() {
        assert_eq!(
            pattern_for_path("spa/src/core/diff.js"),
            "spa/src/core/*.js"
        );
        assert_eq!(pattern_for_path("Cargo.toml"), "*.toml");
        assert_eq!(pattern_for_path(".eslintrc.json"), "*.json");
    }

    /// Some paths generalize to nothing useful: `docs/*` would sweep up every
    /// sibling whatever it is. Those stand for themselves.
    #[test]
    fn a_path_with_no_extension_stands_only_for_itself() {
        assert_eq!(pattern_for_path("docs/README"), "docs/README");
        assert_eq!(pattern_for_path("Makefile"), "Makefile");
        assert_eq!(pattern_for_path(".gitignore"), ".gitignore");
    }

    #[test]
    fn the_same_disagreement_twice_is_one_rule_that_counted_twice() {
        let (document, first) = merge_override(
            Value::Null,
            "spa/src/core/*.js",
            OverrideDirection::Surface,
            "2026-08-14T00:00:00Z",
        )
        .expect("an absent file is an empty document");
        assert_eq!(first, 1);
        assert_eq!(document["version"], 1);

        let (document, second) = merge_override(
            document,
            "spa/src/core/*.js",
            OverrideDirection::Surface,
            "2026-08-14T00:01:00Z",
        )
        .expect("the same pattern merges");
        assert_eq!(second, 2);
        let rules = document["rules"].as_array().expect("rules is a list");
        assert_eq!(rules.len(), 1, "a repeat is a count, not a second rule");
        assert_eq!(rules[0]["count"], 2);
        assert_eq!(rules[0]["direction"], "surface");
        assert_eq!(rules[0]["updated_at"], "2026-08-14T00:01:00Z");
    }

    /// One pattern pulled both ways is two rules. Folding them together would
    /// hide the only thing worth seeing: that the reviewer is of two minds.
    #[test]
    fn one_pattern_pulled_both_ways_is_two_rules() {
        let (document, _) = merge_override(
            Value::Null,
            "bridge/src/*.rs",
            OverrideDirection::Surface,
            "2026-08-14T00:00:00Z",
        )
        .unwrap();
        let (document, count) = merge_override(
            document,
            "bridge/src/*.rs",
            OverrideDirection::Collapse,
            "2026-08-14T00:01:00Z",
        )
        .unwrap();
        assert_eq!(count, 1, "the other direction starts its own count");
        assert_eq!(document["rules"].as_array().unwrap().len(), 2);
    }

    /// The file lives in the repository, so a human may widen a rule or add a
    /// field the bridge has never heard of. A bridge-written count must not be
    /// the thing that eats it.
    #[test]
    fn what_a_human_wrote_into_the_file_survives_a_bridge_written_count() {
        let hand_written = json!({
            "version": 1,
            "note": "hand-tuned; do not sort",
            "rules": [{
                "pattern": "bridge/src/*.rs",
                "path_glob": "bridge/**/*.rs",
                "direction": "surface",
                "count": 3,
                "why": "crypto lives here"
            }],
        });

        let (document, count) = merge_override(
            hand_written,
            "bridge/src/*.rs",
            OverrideDirection::Surface,
            "2026-08-14T00:00:00Z",
        )
        .unwrap();

        assert_eq!(count, 4, "the human's count is continued, not restarted");
        assert_eq!(document["note"], "hand-tuned; do not sort");
        let rule = &document["rules"][0];
        assert_eq!(rule["path_glob"], "bridge/**/*.rs");
        assert_eq!(rule["why"], "crypto lives here");
    }

    #[test]
    fn a_rules_file_that_is_not_a_document_of_rules_is_refused() {
        assert!(merge_override(
            json!([{ "pattern": "*.rs" }]),
            "*.rs",
            OverrideDirection::Surface,
            "2026-08-14T00:00:00Z",
        )
        .is_err());
        assert!(merge_override(
            json!({ "rules": "none" }),
            "*.rs",
            OverrideDirection::Surface,
            "2026-08-14T00:00:00Z",
        )
        .is_err());
    }

    /// An unreadable file is not an empty one: overwriting what a human wrote
    /// because the bridge could not parse it loses the signal outright.
    #[test]
    fn an_unparseable_rules_file_is_an_error_not_a_fresh_start() {
        let dir = tempfile::tempdir().expect("a temp checkout");
        std::fs::create_dir_all(dir.path().join(".build")).unwrap();
        assert_eq!(
            read(dir.path()).expect("no file at all is no document"),
            Value::Null
        );

        std::fs::write(dir.path().join(REVIEW_RULES_PATH), "{ not json").unwrap();
        assert!(read(dir.path()).is_err());
    }

    #[test]
    fn a_written_document_reads_back_as_itself() {
        let dir = tempfile::tempdir().expect("a temp checkout");
        let (document, _) = merge_override(
            Value::Null,
            "spa/src/*.js",
            OverrideDirection::Collapse,
            "2026-08-14T00:00:00Z",
        )
        .unwrap();

        write(dir.path(), &document).expect("the checkout is writable");

        assert_eq!(read(dir.path()).expect("it parses"), document);
    }
}
