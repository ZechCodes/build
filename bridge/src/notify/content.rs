//! What a sealed push says (#200): a title, a body and a same-origin deep
//! link, each cut to the caps the scheme sets
//! (`planning/v2/Push Content Security Checklist.md`).
//!
//! Content is words the user wrote or an agent said, so it never reaches a
//! log: [`PushContent`]'s `Debug` prints lengths, never text, and nothing here
//! formats it into an error.

use serde::Serialize;

/// How long one line may be: at most `chars` characters, and at most
/// `json_bytes` bytes as JSON writes it (a quote two, a control character
/// six), so the sealed plaintext always fits its fixed size.
#[derive(Debug, Clone, Copy)]
pub struct Cap {
    pub chars: usize,
    pub json_bytes: usize,
}

/// The title's cap.
pub const TITLE: Cap = Cap {
    chars: 64,
    json_bytes: 160,
};
/// The body's cap.
pub const BODY: Cap = Cap {
    chars: 160,
    json_bytes: 480,
};
/// A url longer than this says nothing. With the title's and body's caps and
/// the JSON around them this keeps every plaintext inside
/// `seal::PLAINTEXT_MAX_BYTES`.
pub const URL_MAX_BYTES: usize = 320;
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
        if !url.starts_with(APP_URL_PREFIX) || json_bytes(&url) > URL_MAX_BYTES {
            return None;
        }
        Some(PushContent {
            title: one_line(title, TITLE)?,
            body: one_line(body, BODY)?,
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

/// Content as the app lock hands it over: ready, or with a last step that
/// touches the disk (resolving a workspace's path) left for the spawned
/// delivery, which runs it on the blocking pool and only when there is a key
/// to seal to. The app lock is never held for the disk.
pub enum PendingContent {
    Ready(PushContent),
    Deferred(Box<dyn FnOnce() -> Option<PushContent> + Send>),
}

impl PendingContent {
    /// The content, resolving a deferred step here and now.
    pub fn resolve(self) -> Option<PushContent> {
        match self {
            PendingContent::Ready(content) => Some(content),
            PendingContent::Deferred(resolve) => resolve(),
        }
    }
}

impl From<PushContent> for PendingContent {
    fn from(content: PushContent) -> Self {
        PendingContent::Ready(content)
    }
}

/// [`first_line`] cut to `cap` on a character boundary, ending in `…` when it
/// was cut.
pub fn one_line(text: &str, cap: Cap) -> Option<String> {
    let line = first_line(text)?;
    if line.chars().count() <= cap.chars && json_bytes(&line) <= cap.json_bytes {
        return Some(line);
    }
    let mut room = cap.json_bytes - json_bytes(ELLIPSIS);
    let kept: String = line
        .chars()
        .take(cap.chars - 1)
        .take_while(|c| {
            let fits = json_char_bytes(*c) <= room;
            room = room.saturating_sub(json_char_bytes(*c));
            fits
        })
        .collect();
    Some(format!("{}{ELLIPSIS}", kept.trim_end()))
}

const ELLIPSIS: &str = "…";

/// How many bytes `text` takes inside a JSON string, as serde_json writes it.
pub fn json_bytes(text: &str) -> usize {
    text.chars().map(json_char_bytes).sum()
}

fn json_char_bytes(c: char) -> usize {
    match c {
        '"' | '\\' | '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        c if u32::from(c) < 0x20 => 6,
        c => c.len_utf8(),
    }
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
            one_line("ready for review", BODY).as_deref(),
            Some("ready for review")
        );
        let exactly = "x".repeat(64);
        assert_eq!(one_line(&exactly, TITLE), Some(exactly));
    }

    #[test]
    fn a_long_first_line_is_cut_to_the_cap_and_ends_in_an_ellipsis() {
        let long = format!("{}\nsecond", "word ".repeat(100));
        let cut = one_line(&long, BODY).unwrap();
        assert_eq!(cut.chars().count(), BODY.chars, "{cut}");
        assert!(cut.ends_with("word…"), "{cut}");
        // A cut that lands after a space drops it before the ellipsis.
        let spaced = format!("{} tail", "a".repeat(BODY.chars - 2));
        let cut = one_line(&spaced, BODY).unwrap();
        assert!(cut.ends_with("a…"), "{cut}");
        assert_eq!(cut.chars().count(), BODY.chars - 1);
        let dense = "a".repeat(200);
        let cut = one_line(&dense, TITLE).unwrap();
        assert_eq!(cut.chars().count(), TITLE.chars);
        assert!(cut.ends_with('…'));
    }

    #[test]
    fn multibyte_text_is_cut_on_a_character_boundary() {
        let text = "ünïcode ✓ 🚀".repeat(20);
        let cut = one_line(&text, TITLE).unwrap();
        assert!(cut.chars().count() <= TITLE.chars, "{cut}");
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
    fn a_line_is_also_cut_to_its_json_bytes() {
        // A quote is two bytes of JSON, a control character six, a rocket four.
        for (unit, fits) in [("\"", 64), ("\u{1}", 27), ("🚀", 40), ("a", 64)] {
            let cut = one_line(&unit.repeat(200), TITLE).unwrap();
            assert!(json_bytes(&cut) <= TITLE.json_bytes, "{unit:?}");
            assert!(cut.ends_with('…'), "{unit:?}");
            assert_eq!(cut.chars().count(), fits, "{unit:?}");
        }
        let exactly = "🚀".repeat(40);
        assert_eq!(one_line(&exactly, TITLE), Some(exactly));
    }

    #[test]
    fn a_url_past_its_cap_says_nothing() {
        let at_cap = format!(
            "{APP_URL_PREFIX}{}",
            "x".repeat(URL_MAX_BYTES - APP_URL_PREFIX.len())
        );
        assert!(PushContent::new("t", "b", at_cap.clone()).is_some());
        assert!(PushContent::new("t", "b", format!("{at_cap}x")).is_none());
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
