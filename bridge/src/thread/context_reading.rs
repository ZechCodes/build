//! How full an agent's context was when it wrote something (#68).
//!
//! A message an agent sends, and a comment it leaves on a task, carry the
//! author's last reading — snapshotted at write time, because the reader
//! wants to know how much room the author had when it wrote these words, not
//! how much it has by the time somebody looks.

use serde::{Deserialize, Serialize};

/// One agent's context as its harness last reported it: `tokens` in context,
/// the `window` of the model it runs when Build knows it, `compact_at`, the
/// threshold its chat compacts at when it compacts at all, and `at`, the
/// RFC3339 time the reading was recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextReading {
    pub tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compact_at: Option<u64>,
    pub at: String,
}

impl ContextReading {
    /// The one line a reader is given about its author, measured against
    /// where its chat compacts — "Rail scroll is at 190k of 200k (95%,
    /// compacts at 200k)." A chat that never compacts is measured against the
    /// window instead — "Rail scroll is at 612k of 1M (61%)." — and one with
    /// neither says only its size: "Rail scroll is at 612k."
    pub fn sentence(&self, author: &str) -> String {
        let tokens = rounded_tokens(self.tokens);
        if let Some(compact_at) = self.compact_at.filter(|threshold| *threshold > 0) {
            let threshold = rounded_tokens(compact_at);
            let percent = percent_of(self.tokens, compact_at);
            return format!(
                "{author} is at {tokens} of {threshold} ({percent}%, compacts at {threshold})."
            );
        }
        match self.window.filter(|window| *window > 0) {
            Some(window) => format!(
                "{author} is at {tokens} of {} ({}%).",
                rounded_tokens(window),
                percent_of(self.tokens, window)
            ),
            None => format!("{author} is at {tokens}."),
        }
    }
}

/// A token count as a person says it: 640, 612k, 1M, 1.2M.
fn rounded_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        return tokens.to_string();
    }
    let thousands = (tokens + 500) / 1_000;
    if thousands < 1_000 {
        return format!("{thousands}k");
    }
    let tenths_of_a_million = (tokens + 50_000) / 100_000;
    match tenths_of_a_million % 10 {
        0 => format!("{}M", tenths_of_a_million / 10),
        tenth => format!("{}.{tenth}M", tenths_of_a_million / 10),
    }
}

/// The share of `window` that `tokens` is, to the nearest whole percent.
fn percent_of(tokens: u64, window: u64) -> u64 {
    (tokens * 100 + window / 2) / window
}

#[cfg(test)]
mod tests;
