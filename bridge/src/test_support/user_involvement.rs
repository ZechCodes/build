//! When an agent involves the user (#232), one sentence per rule. Every
//! surface that carries a rule — the conversation protocol, the tool
//! descriptions, the task-tools note — carries it in exactly these words, so
//! the rule reads the same wherever an agent meets it.

/// Where the user is talked to once they are on a task.
pub const ON_A_TASK: &str = "Once the user is on a task (they filed it, commented on it, were asked on it, or asked to follow it), talk to them about it on the task with comment_task: questions, results and meaningful progress, not every step.";
/// What they write on a task is answered there.
pub const REPLY_THERE: &str = "When they write on it, reply there.";
/// A question is answered where it was asked.
pub const ASKED_IN_THREAD: &str =
    "A question asked in your thread is answered in your thread, even when it is about a task.";
/// A result that is on a task is not reported twice.
pub const RESULT_ON_TASK: &str =
    "When the result is on a task the user is on, the Complete in your thread is one line that points to it.";
/// The whole-report rule's one exception, where the report rule is stated.
pub const WHOLE_REPORT_UNLESS: &str =
    "the body carries the whole of it unless the result is on a task the user is on.";
/// A comment's `notify_user`: feedback, never progress on a followed task.
pub const COMMENT_NOTIFY: &str = "On a comment, notify_user is only for feedback you need from the user; leave it off for progress on a task the user follows, since watching already shows it.";
/// A reply to the user builds on what they wrote.
pub const BUILD_ON: &str = "When you answer what the user wrote, move it forward: settle the question, make the call, or add the detail that was missing. Do not quote it or say it back.";
/// The two task flags, told apart.
pub const FLAG_SPLIT: &str =
    "mention_user asks; notify_user is only for a task the user asked to follow.";
/// A task's `notify_user`.
pub const WATCH_TASK: &str = "Off by default: pass true only when the user asked to follow it.";
/// A report is not a request to follow.
pub const REPORTED: &str =
    "A task filed from something the user reported is not one they asked to follow.";
/// A new agent's `notify_user`.
pub const WATCH_AGENT: &str = "Off by default: pass true only when the user asked to follow its work; starting work they asked for is not that.";

/// Whether `text` states `rule`, across line wrapping and code spans.
pub fn says(text: &str, rule: &str) -> bool {
    let plain = text.replace('`', "");
    plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .contains(rule)
}
