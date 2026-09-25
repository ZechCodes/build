//! `issues.list` a page at a time (#85).
//!
//! The list's order is number descending, and a number is minted once and
//! never moves, so the last number a page answered is a place in that order
//! that no later write can shift: the next page is the issues below it. An
//! issue filed meanwhile lands above the first page, where the client's next
//! read from the top finds it.
//!
//! The cursor carries that number, a digest of the list it was made in and a
//! digest of the filter it was made under. A cursor is only a place in ONE
//! list: below #40 of this device's Build project is not below #40 of another
//! device's, whose project may wear the same boot-local `proj-N`, and below
//! #40 of the open issues is not below #40 of the closed ones. So the list is
//! the store's own key and the project's repository path, and a cursor handed
//! back to another store, another project or under another filter is refused
//! rather than answered as a page of a list nobody asked for. Opaque to the
//! client: it is handed back as it came.

use super::edits::AssigneeFilter;
use crate::tracker::IssueState;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// The most issues one page holds.
const MOST_PER_PAGE: u64 = 500;

/// How many rows a page reads for each row it may answer. A filter the
/// store cannot seek on — an assignee, a label — passes over rows it does not
/// keep; past this many a page stops short and names where the next one
/// starts, so a label nobody carries costs a bounded read, not the whole
/// project under the lock.
const ROWS_READ_PER_ROW: usize = 4;

/// The version of the cursor's spelling, so a later one can be told apart.
const CURSOR_VERSION: &str = "v1";

const UNREADABLE: &str = "Build cannot read this cursor: ask for the list again from the start.";
const ANOTHER_FILTER: &str =
    "Build cannot continue this list: the cursor was made for a different filter.";
const ANOTHER_LIST: &str =
    "Build cannot continue this list: the cursor was made for another project or on another device.";

/// Which list a read is of: the store that holds it, by the key it keeps for
/// its cursors, and the project, by its repository path — the name that
/// outlives a restart, where a `proj-N` does not.
pub(super) struct ListOf<'a> {
    pub store_key: &'a str,
    pub project_path: &'a str,
}

impl ListOf<'_> {
    fn digest(&self) -> String {
        digest_of(&json!([self.store_key, self.project_path]))
    }
}

/// What one list read narrows by, as it MEANS it rather than as it was typed:
/// a column named the way it is shown and the way it is stored is one
/// filter, and so is a label in either case.
pub(super) struct ListFilter<'a> {
    pub state: Option<IssueState>,
    pub status: Option<&'a str>,
    pub assignee: &'a AssigneeFilter,
    pub label: Option<&'a str>,
}

impl ListFilter<'_> {
    /// A digest of the filter, stable across restarts and releases of the
    /// bridge, which a cursor carries so it can only continue its own list.
    fn digest(&self) -> String {
        digest_of(&json!([
            self.state.map(IssueState::as_str),
            self.status,
            self.assignee.key(),
            self.label.map(str::to_ascii_lowercase),
        ]))
    }
}

/// The first eight bytes of a meaning's SHA-256, in hex: enough to tell two
/// lists or two filters apart, short enough to keep a cursor short.
fn digest_of(meaning: &Value) -> String {
    let digest = Sha256::digest(meaning.to_string().as_bytes());
    digest[..8]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Where a cursor was made: the list and the filter, each as its digest.
#[derive(PartialEq)]
struct MadeIn {
    list: String,
    filter: String,
}

/// One page, as it was asked for: how many rows at most, and the number the
/// rows are all below. Neither is a whole-list read.
pub(super) struct PageAsk {
    pub limit: Option<usize>,
    pub below: Option<u64>,
    made_in: MadeIn,
}

impl PageAsk {
    pub(super) fn parse(
        params: &Value,
        list: &ListOf<'_>,
        filter: &ListFilter<'_>,
    ) -> Result<Self, String> {
        let made_in = MadeIn {
            list: list.digest(),
            filter: filter.digest(),
        };
        let below = match params.get("cursor") {
            None | Some(Value::Null) => None,
            Some(cursor) => Some(cursor_number(cursor, &made_in)?),
        };
        Ok(Self {
            limit: limit(params)?,
            below,
            made_in,
        })
    }

    /// How many rows to keep to know whether there is a page after this one.
    pub(super) fn rows_to_keep(&self) -> Option<usize> {
        self.limit.map(|limit| limit + 1)
    }

    /// How many rows the page may read to keep them.
    pub(super) fn rows_to_scan(&self) -> Option<usize> {
        self.rows_to_keep().map(|keep| keep * ROWS_READ_PER_ROW)
    }

    /// Cut what was kept to the page, and name the next page when there is
    /// one. `rows` holds at most [`Self::rows_to_keep`] issues; `scanned_to`
    /// is the last number read when the read stopped at
    /// [`Self::rows_to_scan`] with rows still below it, and then the page is
    /// short, or empty, and the next starts below that number.
    pub(super) fn cut<T>(
        &self,
        rows: &mut Vec<T>,
        scanned_to: Option<u64>,
        number: impl Fn(&T) -> u64,
    ) -> Option<String> {
        let limit = self.limit?;
        if rows.len() <= limit {
            return scanned_to.map(|last| cursor_for(last, &self.made_in));
        }
        rows.truncate(limit);
        rows.last()
            .map(|last| cursor_for(number(last), &self.made_in))
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

fn cursor_for(number: u64, made_in: &MadeIn) -> String {
    let MadeIn { list, filter } = made_in;
    URL_SAFE_NO_PAD.encode(format!("{CURSOR_VERSION}:{number}:{list}:{filter}"))
}

/// The number a cursor continues below, once it is known to be one this
/// list and this filter made.
fn cursor_number(cursor: &Value, made_in: &MadeIn) -> Result<u64, String> {
    let (number, made) = cursor
        .as_str()
        .and_then(|cursor| URL_SAFE_NO_PAD.decode(cursor).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .and_then(|spelled| read_cursor(&spelled))
        .ok_or_else(|| UNREADABLE.to_string())?;
    if made.list != made_in.list {
        return Err(ANOTHER_LIST.to_string());
    }
    if made.filter != made_in.filter {
        return Err(ANOTHER_FILTER.to_string());
    }
    Ok(number)
}

fn read_cursor(spelled: &str) -> Option<(u64, MadeIn)> {
    let mut parts = spelled.split(':');
    if parts.next()? != CURSOR_VERSION {
        return None;
    }
    let number = parts.next()?.parse().ok()?;
    let mut digest = || {
        parts
            .next()
            .filter(|digest| !digest.is_empty())
            .map(str::to_string)
    };
    let made = MadeIn {
        list: digest()?,
        filter: digest()?,
    };
    parts.next().is_none().then_some((number, made))
}
