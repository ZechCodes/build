//! `issues.list` a page at a time (#85).
//!
//! The list's order is number descending, and a number is minted once and
//! never moves, so the last number a page answered is a place in that order
//! that no later write can shift: the next page is the issues below it. An
//! issue filed meanwhile lands above the first page, where the client's next
//! read from the top finds it.
//!
//! The cursor carries that number and a digest of the filter it was made
//! under. A cursor is only a place in ONE filter's list — below #40 of the
//! open issues is not below #40 of the closed ones — so a cursor handed back
//! with another filter is refused rather than answered as a page of a list
//! nobody asked for. Opaque to the client: it is handed back as it came.

use super::edits::AssigneeFilter;
use crate::tracker::IssueState;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The most issues one page holds.
const MOST_PER_PAGE: u64 = 500;

/// The version of the cursor's spelling, so a later one can be told apart.
const CURSOR_VERSION: &str = "v1";

const UNREADABLE: &str = "Build cannot read this cursor: ask for the list again from the start.";
const ANOTHER_FILTER: &str =
    "Build cannot continue this list: the cursor was made for a different filter.";

/// What one list read narrows by, as it MEANS it rather than as it was typed:
/// a column named the way it is shown and the way it is stored is one
/// filter, and so is a label in either case.
pub(super) struct ListFilter<'a> {
    pub project_id: &'a str,
    pub state: Option<IssueState>,
    pub status: Option<&'a str>,
    pub assignee: &'a AssigneeFilter,
    pub label: Option<&'a str>,
}

impl ListFilter<'_> {
    /// A digest of the filter, stable across restarts and releases of the
    /// bridge, which a cursor carries so it can only continue its own list.
    fn digest(&self) -> String {
        let meaning = json!([
            self.project_id,
            self.state.map(IssueState::as_str),
            self.status,
            self.assignee.key(),
            self.label.map(str::to_ascii_lowercase),
        ]);
        let digest = Sha256::digest(meaning.to_string().as_bytes());
        digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
}

/// One page, as it was asked for: how many rows at most, and the number the
/// rows are all below. Neither is a whole-list read.
pub(super) struct PageAsk {
    pub limit: Option<usize>,
    pub below: Option<u64>,
    digest: String,
}

impl PageAsk {
    pub(super) fn parse(params: &Value, filter: &ListFilter<'_>) -> Result<Self, String> {
        let digest = filter.digest();
        let below = match params.get("cursor") {
            None | Some(Value::Null) => None,
            Some(cursor) => Some(cursor_number(cursor, &digest)?),
        };
        Ok(Self {
            limit: limit(params)?,
            below,
            digest,
        })
    }

    /// How many rows to read to know whether there is a page after this one.
    pub(super) fn rows_to_read(&self) -> Option<usize> {
        self.limit.map(|limit| limit + 1)
    }

    /// Cut what was read to the page, and name the next page when there is
    /// one. `rows` holds at most [`Self::rows_to_read`] issue numbers' worth.
    pub(super) fn cut<T>(&self, rows: &mut Vec<T>, number: impl Fn(&T) -> u64) -> Option<String> {
        let limit = self.limit?;
        if rows.len() <= limit {
            return None;
        }
        rows.truncate(limit);
        rows.last()
            .map(|last| cursor_for(number(last), &self.digest))
    }
}

fn limit(params: &Value) -> Result<Option<usize>, String> {
    let Some(asked) = params.get("limit").filter(|asked| !asked.is_null()) else {
        return Ok(None);
    };
    asked
        .as_u64()
        .filter(|limit| (1..=MOST_PER_PAGE).contains(limit))
        .map(|limit| Some(limit as usize))
        .ok_or_else(|| {
            format!(
                "Build cannot list {asked} issues at a time: a page holds 1 to {MOST_PER_PAGE}."
            )
        })
}

fn cursor_for(number: u64, digest: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{CURSOR_VERSION}:{number}:{digest}"))
}

/// The number a cursor continues below, once it is known to be one this
/// filter's list made.
fn cursor_number(cursor: &Value, digest: &str) -> Result<u64, String> {
    let (number, made_under) = cursor
        .as_str()
        .and_then(|cursor| URL_SAFE_NO_PAD.decode(cursor).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|spelled| read_cursor(&spelled))
        .ok_or_else(|| UNREADABLE.to_string())?;
    if made_under != digest {
        return Err(ANOTHER_FILTER.to_string());
    }
    Ok(number)
}

fn read_cursor(spelled: &str) -> Option<(u64, String)> {
    let mut parts = spelled.splitn(3, ':');
    if parts.next()? != CURSOR_VERSION {
        return None;
    }
    let number = parts.next()?.parse().ok()?;
    let digest = parts.next().filter(|digest| !digest.is_empty())?;
    Some((number, digest.to_string()))
}
