//! What a sealed push says (#200): a title, a body and a same-origin deep
//! link, each cut to the caps the scheme sets
//! (`planning/v2/Push Content Security Checklist.md`).
//!
//! Content is words the user wrote or an agent said, so it never reaches a
//! log: [`PushContent`]'s `Debug` prints lengths, never text, and nothing here
//! formats it into an error.

use serde::Serialize;

/// At most this many characters of title.
pub const TITLE_MAX_CHARS: usize = 64;
/// At most this many characters of body.
pub const BODY_MAX_CHARS: usize = 160;
/// Every sealed url starts here: the service worker and the app refuse any
/// other.
pub const APP_URL_PREFIX: &str = "/app/#/";

/// One push's words, already cut to their caps.
#[derive(Clone, PartialEq, Eq, Serialize)]
pub struct PushContent {
    pub title: String,
    pub body: String,
    pub url: String,
}

impl std::fmt::Debug for PushContent {
    /// Lengths only: a stray `{:?}` must not put the words in a log.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PushContent")
            .field("title_chars", &self.title.chars().count())
            .field("body_chars", &self.body.chars().count())
            .field("url_chars", &self.url.chars().count())
            .finish()
    }
}

impl PushContent {
    /// Content out of its sources: the first non-empty line of each, cut to
    /// its cap. `None` when either says nothing or the url leaves the app —
    /// a push without content goes out generic.
    pub fn new(title: &str, body: &str, url: String) -> Option<PushContent> {
        if !url.starts_with(APP_URL_PREFIX) {
            return None;
        }
        Some(PushContent {
            title: one_line(title, TITLE_MAX_CHARS)?,
            body: one_line(body, BODY_MAX_CHARS)?,
            url,
        })
    }
}

/// The first non-empty line of `text`, whitespace collapsed, or `None` when
/// every line is blank.
pub fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .find(|line| !line.is_empty())
}

/// [`first_line`] cut to `max_chars` characters on a character boundary,
/// ending in `…` when it was cut.
pub fn one_line(text: &str, max_chars: usize) -> Option<String> {
    let line = first_line(text)?;
    if line.chars().count() <= max_chars {
        return Some(line);
    }
    let kept: String = line.chars().take(max_chars.saturating_sub(1)).collect();
    Some(format!("{}…", kept.trim_end()))
}

/// A path segment as JS `encodeURIComponent` writes it: everything but
/// `A-Z a-z 0-9 - _ . ! ~ * ' ( )` percent-encoded, byte by byte.
pub fn encode_uri_component(text: &str) -> String {
    percent_encode(text, |byte| {
        byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte)
    })
}

/// A query value as `URLSearchParams` writes it (the SPA router's
/// `hashQuery`): everything but `A-Z a-z 0-9 * - . _` percent-encoded, and a
/// space as `+`.
pub fn encode_query_value(text: &str) -> String {
    percent_encode(text, |byte| {
        byte.is_ascii_alphanumeric() || b"*-._".contains(&byte)
    })
    .replace("%20", "+")
}

fn percent_encode(text: &str, keeps: impl Fn(u8) -> bool) -> String {
    text.bytes()
        .map(|byte| {
            if keeps(byte) {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// Where a conversation lives, as the SPA's router names it.
pub enum ConversationPlace<'a> {
    Workspace {
        project_id: &'a str,
        workspace_id: &'a str,
    },
    Project {
        project_id: &'a str,
    },
}

/// The deep link to one agent's conversation, written exactly as the SPA's
/// router writes it (`spa/src/core/router.js`, `conversationRoute`).
pub fn conversation_url(device_id: &str, place: &ConversationPlace, agent_id: &str) -> String {
    let agent = encode_query_value(agent_id);
    match place {
        ConversationPlace::Workspace {
            project_id,
            workspace_id,
        } => format!(
            "{}/workspace/{}/changes?agent={agent}",
            project_prefix(device_id, project_id),
            encode_uri_component(workspace_id),
        ),
        ConversationPlace::Project { project_id } => {
            format!("{}?agent={agent}", project_prefix(device_id, project_id))
        }
    }
}

fn project_prefix(device_id: &str, project_id: &str) -> String {
    format!(
        "{APP_URL_PREFIX}device/{}/project/{}",
        encode_uri_component(device_id),
        encode_uri_component(project_id)
    )
}

/// The deep link to one tracker task.
pub fn task_url(task_id: &str) -> String {
    format!("{APP_URL_PREFIX}tasks/{}", encode_uri_component(task_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_the_first_non_empty_one_with_whitespace_collapsed() {
        assert_eq!(
            first_line("\n  \n  the   fix\tis in \nsecond line").as_deref(),
            Some("the fix is in")
        );
        assert_eq!(first_line(" \n\t\n"), None);
        assert_eq!(first_line(""), None);
    }

    #[test]
    fn a_short_line_is_kept_whole() {
        assert_eq!(
            one_line("ready for review", 160).as_deref(),
            Some("ready for review")
        );
        let exactly = "x".repeat(64);
        assert_eq!(one_line(&exactly, 64), Some(exactly));
    }

    #[test]
    fn a_long_first_line_is_cut_to_the_cap_and_ends_in_an_ellipsis() {
        let long = format!("{}\nsecond", "word ".repeat(100));
        let cut = one_line(&long, BODY_MAX_CHARS).unwrap();
        assert_eq!(cut.chars().count(), BODY_MAX_CHARS, "{cut}");
        assert!(cut.ends_with("word…"), "{cut}");
        // A cut that lands after a space drops it before the ellipsis.
        let spaced = format!("{} tail", "a".repeat(BODY_MAX_CHARS - 2));
        let cut = one_line(&spaced, BODY_MAX_CHARS).unwrap();
        assert!(cut.ends_with("a…"), "{cut}");
        assert_eq!(cut.chars().count(), BODY_MAX_CHARS - 1);
        let dense = "a".repeat(200);
        let cut = one_line(&dense, TITLE_MAX_CHARS).unwrap();
        assert_eq!(cut.chars().count(), TITLE_MAX_CHARS);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn multibyte_text_is_cut_on_a_character_boundary() {
        let text = "ünïcode ✓ 🚀".repeat(20);
        let cut = one_line(&text, TITLE_MAX_CHARS).unwrap();
        assert!(cut.chars().count() <= TITLE_MAX_CHARS, "{cut}");
        assert!(cut.ends_with('…'));
        assert!(text.starts_with(cut.trim_end_matches('…')));
    }

    #[test]
    fn content_needs_a_title_a_body_and_an_app_url() {
        let url = || "/app/#/tasks/task-1".to_string();
        let content = PushContent::new("#4 Title", "Agent: hi", url()).unwrap();
        assert_eq!(content.title, "#4 Title");
        assert_eq!(content.body, "Agent: hi");
        assert!(PushContent::new("#4 Title", "  \n ", url()).is_none());
        assert!(PushContent::new("", "body", url()).is_none());
        assert!(PushContent::new("t", "b", "https://evil.example/".into()).is_none());
        assert!(PushContent::new("t", "b", "/app/other".into()).is_none());
    }

    #[test]
    fn debug_never_prints_the_words() {
        let content = PushContent::new("secret title", "secret body", task_url("task-1")).unwrap();
        let printed = format!("{content:?}");
        assert!(!printed.contains("secret"), "{printed}");
        assert!(!printed.contains("task-1"), "{printed}");
    }

    #[test]
    fn segments_encode_like_encode_uri_component() {
        assert_eq!(encode_uri_component("proj-1"), "proj-1");
        assert_eq!(
            encode_uri_component("a b/c?d#é~*'()!"),
            "a%20b%2Fc%3Fd%23%C3%A9~*'()!"
        );
        assert_eq!(encode_query_value("a b~!"), "a+b%7E%21");
    }

    #[test]
    fn a_workspace_agents_link_opens_its_workspace_changes_with_the_agent() {
        let place = ConversationPlace::Workspace {
            project_id: "proj-1",
            workspace_id: "ws 1",
        };
        assert_eq!(
            conversation_url("dev-1", &place, "agent-1"),
            "/app/#/device/dev-1/project/proj-1/workspace/ws%201/changes?agent=agent-1"
        );
    }

    #[test]
    fn a_project_agents_link_opens_the_project_with_the_agent() {
        let place = ConversationPlace::Project {
            project_id: "proj/2",
        };
        assert_eq!(
            conversation_url("dev 1", &place, "agent-7"),
            "/app/#/device/dev%201/project/proj%2F2?agent=agent-7"
        );
    }

    #[test]
    fn a_task_link_names_the_task() {
        assert_eq!(task_url("task-01ABC"), "/app/#/tasks/task-01ABC");
    }
}
