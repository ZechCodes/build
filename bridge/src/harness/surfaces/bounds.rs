pub const GOAL_OBJECTIVE_LIMIT: usize = 4 * 1024;
pub const CHECKLIST_TEXT_LIMIT: usize = 4 * 1024;
pub const PROVIDER_TOKEN_LIMIT: usize = 1024;
pub const PLAN_EXPLANATION_LIMIT: usize = 8 * 1024;
pub const CHECKLIST_ITEM_LIMIT: usize = 256;
pub const TERMINAL_EXECUTION_ITEM_LIMIT: usize = 128;

pub fn bounded(value: impl Into<String>, limit: usize) -> (String, bool) {
    let value = value.into();
    if value.len() <= limit {
        return (value, false);
    }
    let mut end = limit;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    (value[..end].to_string(), true)
}

pub fn bounded_optional(value: Option<String>, limit: usize) -> (Option<String>, bool) {
    match value {
        Some(value) => {
            let (value, truncated) = bounded(value, limit);
            (Some(value), truncated)
        }
        None => (None, false),
    }
}
