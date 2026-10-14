//! Provider error classification and bounded text safe to persist or log.

use std::ops::Range;

use serde_json::Value;

use super::protocol::NotificationError;

const MAX_ERROR_BYTES: usize = 1024;
const REDACTED: &str = "[REDACTED]";
const ERROR_CODES: &[&str] = &[
    "contextWindowExceeded",
    "usageLimitExceeded",
    "httpConnectionFailed",
    "responseStreamConnectionFailed",
    "responseStreamDisconnected",
    "responseTooManyFailedAttempts",
    "badRequest",
    "unauthorized",
    "sandboxError",
    "internalServerError",
    "other",
];

impl NotificationError {
    /// A transport/server failure may be retried only after account and HTTP
    /// refusals have been ruled out. Unknown codes never opt into text fallback.
    pub fn is_transient(&self) -> bool {
        if self.has_refusal_hint() {
            return false;
        }
        let code = self.code();
        if !allows_retry(code, self.codex_error_info.as_ref()) {
            return false;
        }
        let statuses = self.http_statuses();
        if statuses.iter().any(|status| !transient_status(*status)) {
            return false;
        }
        if !statuses.is_empty() {
            return true;
        }
        match code {
            Some(
                "internalServerError"
                | "responseStreamConnectionFailed"
                | "responseStreamDisconnected"
                | "responseTooManyFailedAttempts",
            ) => true,
            Some("httpConnectionFailed") => false,
            _ => self.texts().any(explicit_transient_hint),
        }
    }

    pub fn safe_message(&self) -> String {
        sanitize_error(&self.message)
    }

    /// Never render arbitrary keys or values supplied in codexErrorInfo.
    pub fn safe_code(&self) -> Option<String> {
        self.code().map(str::to_string)
    }

    pub fn safe_http_status(&self) -> Option<u16> {
        self.http_statuses()
            .into_iter()
            .find(|status| (100..=599).contains(status))
    }

    fn code(&self) -> Option<&'static str> {
        let info = self.codex_error_info.as_ref()?;
        let mut codes = ERROR_CODES.iter().copied().filter(|code| {
            info.as_str() == Some(*code)
                || info
                    .as_object()
                    .is_some_and(|object| object.contains_key(*code))
        });
        let code = codes.next()?;
        codes.next().is_none().then_some(code)
    }

    fn texts(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.message.as_str()).chain(self.additional_details.as_deref())
    }

    fn has_refusal_hint(&self) -> bool {
        self.texts().any(refusal_hint)
    }

    fn http_statuses(&self) -> Vec<u16> {
        let mut statuses = Vec::new();
        if let Some(info) = &self.codex_error_info {
            collect_http_statuses(info, &mut statuses);
        }
        for text in self.texts() {
            statuses.extend(text_http_statuses(text));
        }
        statuses
    }
}

fn allows_retry(code: Option<&str>, info: Option<&Value>) -> bool {
    match code {
        Some(
            code @ ("httpConnectionFailed"
            | "responseStreamConnectionFailed"
            | "responseStreamDisconnected"
            | "responseTooManyFailedAttempts"
            | "internalServerError"),
        ) => info.is_some_and(|info| recognized_retry_shape(info, code)),
        None => info.is_none_or(Value::is_null),
        _ => false,
    }
}

fn recognized_retry_shape(info: &Value, code: &str) -> bool {
    match info {
        Value::String(_) => true,
        Value::Object(object) => object.iter().all(|(key, value)| {
            if key == code {
                recognized_variant_payload(value)
            } else {
                key == "httpStatusCode" && nullable_http_status(value)
            }
        }),
        _ => false,
    }
}

fn recognized_variant_payload(value: &Value) -> bool {
    value.as_object().is_some_and(|object| {
        object
            .iter()
            .all(|(key, value)| key == "httpStatusCode" && nullable_http_status(value))
    })
}

fn nullable_http_status(value: &Value) -> bool {
    value.is_null()
        || value
            .as_u64()
            .is_some_and(|status| (100..=599).contains(&status))
}

fn transient_status(status: u16) -> bool {
    status == 408 || (500..=599).contains(&status)
}

fn collect_http_statuses(value: &Value, statuses: &mut Vec<u16>) {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if key == "httpStatusCode" {
                    if let Some(status) =
                        value.as_u64().and_then(|status| u16::try_from(status).ok())
                    {
                        statuses.push(status);
                    }
                } else {
                    collect_http_statuses(value, statuses);
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_http_statuses(value, statuses);
            }
        }
        _ => {}
    }
}

fn text_http_statuses(text: &str) -> Vec<u16> {
    let lower = text.to_ascii_lowercase();
    let mut previous = "";
    lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .filter_map(|word| {
            let status = matches!(previous, "http" | "status" | "code" | "httpstatuscode")
                .then(|| word.parse::<u16>().ok())
                .flatten()
                .filter(|status| (100..=599).contains(status));
            previous = word;
            status
        })
        .collect()
}

fn refusal_hint(text: &str) -> bool {
    let normalized = text
        .to_ascii_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    [
        "context window",
        "context length",
        "contextwindow",
        "contextlength",
        "quota",
        "usage limit",
        "usagelimit",
        "rate limit",
        "ratelimit",
        "too many requests",
        "unauthorized",
        "unauthorised",
        "forbidden",
        "authentication",
        "authorization",
        "auth failed",
        "invalid api key",
        "invalid token",
        "token expired",
        "expired token",
        "invalid credential",
        "permission denied",
        "insufficient credits",
        "insufficient funds",
        "sign in",
        "login",
        "log in",
    ]
    .iter()
    .any(|hint| normalized.contains(hint))
}

fn explicit_transient_hint(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "overloaded",
        "overload",
        "temporarily unavailable",
    ]
    .iter()
    .any(|hint| lower.contains(hint))
}

/// Persist only the human prefix before a structure or key/value delimiter,
/// redacting credentials before bounding. Provider payload text never follows
/// the fixed marker, including when this sanitizer is called again.
pub(super) fn sanitize_error(text: &str) -> String {
    let prefix = human_message_prefix(text);
    let normalized: String = prefix
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let fragments = fragments(&normalized);
    let mut secrets = Vec::new();
    for fragment in &fragments {
        let word = &normalized[fragment.clone()];
        if secret_shape(word) {
            secrets.push(fragment.clone());
        }
        if credential_label(word) {
            if let Some(value) = credential_value(&normalized, fragment.end, word) {
                secrets.push(value);
            }
        }
    }
    let sanitized = replace_secrets(&normalized, secrets);
    bounded_text(&human_message_prefix(&sanitized))
}

fn human_message_prefix(text: &str) -> String {
    let Some(end) = text.find(['{', '[', '<', '=', ':']) else {
        return text.to_string();
    };
    let prefix = text[..end].trim_end();
    if prefix.is_empty() {
        REDACTED.to_string()
    } else {
        format!("{prefix} {REDACTED}")
    }
}

fn label_value_start(text: &str, after: usize) -> usize {
    let after = after + usize::from(text[after..].starts_with(['\'', '"']));
    skip_value_separators(text, after)
}

fn boundary(c: char) -> bool {
    c.is_whitespace()
        || matches!(
            c,
            '\'' | '"' | ':' | '=' | ',' | ';' | '{' | '}' | '[' | ']' | '(' | ')' | '&' | '?'
        )
}

fn fragments(text: &str) -> Vec<Range<usize>> {
    let mut fragments = Vec::new();
    let mut start = None;
    for (offset, c) in text.char_indices() {
        if boundary(c) {
            if let Some(start) = start.take() {
                fragments.push(start..offset);
            }
        } else {
            start.get_or_insert(offset);
        }
    }
    if let Some(start) = start {
        fragments.push(start..text.len());
    }
    fragments
}

fn secret_shape(word: &str) -> bool {
    [
        "sk-",
        "ghp_",
        "github_pat_",
        "glpat-",
        "xoxb-",
        "xoxp-",
        "AKIA",
    ]
    .iter()
    .any(|prefix| word.contains(prefix))
        || (word.starts_with("eyJ") && word.split('.').count() == 3)
}

fn credential_label(word: &str) -> bool {
    let lower = word.to_ascii_lowercase().replace('-', "_");
    matches!(
        lower.as_str(),
        "authorization"
            | "bearer"
            | "password"
            | "passwd"
            | "secret"
            | "token"
            | "key"
            | "api_key"
            | "apikey"
            | "access_token"
            | "refresh_token"
            | "client_secret"
    ) || lower.ends_with("token")
        || lower.ends_with("_key")
}

fn credential_value(text: &str, after: usize, label: &str) -> Option<Range<usize>> {
    let mut cursor = label_value_start(text, after);
    if label.eq_ignore_ascii_case("authorization") {
        let scheme_end = text[cursor..]
            .find(boundary)
            .map_or(text.len(), |end| cursor + end);
        let scheme = &text[cursor..scheme_end];
        if scheme.eq_ignore_ascii_case("bearer") || scheme.eq_ignore_ascii_case("basic") {
            cursor = skip_value_separators(text, scheme_end);
        }
    }
    value_range(text, cursor)
}

fn skip_value_separators(text: &str, start: usize) -> usize {
    text[start..]
        .char_indices()
        .find(|(_, c)| !c.is_whitespace() && !matches!(c, ':' | '='))
        .map_or(text.len(), |(offset, _)| start + offset)
}

fn value_range(text: &str, start: usize) -> Option<Range<usize>> {
    let first = text[start..].chars().next()?;
    if matches!(first, '\'' | '"') {
        let value_start = start + 1;
        let end = quoted_end(text, value_start, first);
        return Some(value_start..end);
    }
    let end = text[start..]
        .find(boundary)
        .map_or(text.len(), |end| start + end);
    (end > start).then_some(start..end)
}

fn quoted_end(text: &str, start: usize, quote: char) -> usize {
    let mut escaped = false;
    text[start..]
        .char_indices()
        .find_map(|(offset, c)| {
            if escaped {
                escaped = false;
                None
            } else if c == '\\' {
                escaped = true;
                None
            } else {
                (c == quote).then_some(start + offset)
            }
        })
        .unwrap_or(text.len())
}

fn replace_secrets(text: &str, mut secrets: Vec<Range<usize>>) -> String {
    secrets.sort_by_key(|secret| secret.start);
    let mut sanitized = String::new();
    let mut after = 0;
    for secret in secrets {
        if secret.start >= after {
            sanitized.push_str(&text[after..secret.start]);
            sanitized.push_str(REDACTED);
        }
        after = after.max(secret.end);
    }
    sanitized.push_str(&text[after..]);
    sanitized
}

fn bounded_text(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut end = text.len().min(MAX_ERROR_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::super::protocol::{NotificationError, ServerNotification};
    use super::sanitize_error;

    fn error(message: &str, info: Value, details: Option<&str>) -> NotificationError {
        serde_json::from_value(json!({
            "message": message,
            "codexErrorInfo": info,
            "additionalDetails": details,
        }))
        .unwrap()
    }

    #[test]
    fn decodes_structured_error_fields_and_completion_details() {
        let expected = error(
            "stream failed",
            json!({"responseStreamDisconnected":{"httpStatusCode":503}}),
            Some("upstream temporarily unavailable"),
        );
        assert_eq!(
            expected.additional_details.as_deref(),
            Some("upstream temporarily unavailable")
        );
        assert_eq!(
            expected.safe_code().as_deref(),
            Some("responseStreamDisconnected")
        );
        assert_eq!(expected.safe_http_status(), Some(503));

        let ServerNotification::TurnCompleted { completion, .. } = ServerNotification::decode(
            "turn/completed",
            json!({"threadId":"thread", "turn": {
                "id":"turn", "status":"failed", "error":{
                    "message":"  stream\nfailed  ",
                    "codexErrorInfo":{"responseStreamDisconnected":{"httpStatusCode":503}},
                    "additionalDetails":"upstream temporarily unavailable"
                }
            }}),
        )
        .unwrap() else {
            panic!("expected completion");
        };
        assert_eq!(completion.error.as_deref(), Some("stream failed"));
        assert_eq!(completion.details, Some(expected));
    }

    #[test]
    fn retries_transport_and_server_failure_variants() {
        for info in [
            json!("responseStreamConnectionFailed"),
            json!({"responseStreamDisconnected":{"httpStatusCode":null}}),
            json!({"responseTooManyFailedAttempts":{"httpStatusCode":null}}),
            json!("internalServerError"),
            json!({"httpConnectionFailed":{"httpStatusCode":408}}),
            json!({"httpConnectionFailed":{"httpStatusCode":502}}),
            json!({"httpConnectionFailed":{},"httpStatusCode":503}),
        ] {
            assert!(
                error("provider failed", info.clone(), None).is_transient(),
                "{info}"
            );
        }
    }

    #[test]
    fn rejects_known_nontransient_and_unknown_codes_despite_timeout_text() {
        for code in [
            "contextWindowExceeded",
            "usageLimitExceeded",
            "badRequest",
            "unauthorized",
            "sandboxError",
            "httpConnectionFailed",
            "futureProviderError",
        ] {
            assert!(
                !error("request timed out", json!(code), None).is_transient(),
                "{code}"
            );
        }
        assert!(!error(
            "timeout",
            json!({"responseStreamDisconnected":{}, "sandboxError":{}}),
            None
        )
        .is_transient());
    }

    #[test]
    fn nontransient_http_status_vetoes_wrapped_stream_errors() {
        for status in [400, 401, 403, 404, 409, 429] {
            for code in [
                "responseStreamConnectionFailed",
                "responseStreamDisconnected",
                "responseTooManyFailedAttempts",
                "internalServerError",
            ] {
                let info = json!({code:{"httpStatusCode":status}});
                assert!(
                    !error("request timed out", info, None).is_transient(),
                    "{code}: {status}"
                );
            }
        }
        assert!(!error(
            "timeout",
            json!({"responseStreamDisconnected":{"httpStatusCode":503}, "httpStatusCode":429}),
            None
        )
        .is_transient());
    }

    #[test]
    fn malformed_http_status_never_opts_into_retry() {
        for status in [
            json!("401"),
            json!("503"),
            json!(-401),
            json!(true),
            json!(408.5),
            json!({}),
            json!([]),
            json!(0),
            json!(600),
        ] {
            for code in [
                "responseStreamDisconnected",
                "responseStreamConnectionFailed",
                "responseTooManyFailedAttempts",
                "internalServerError",
                "other",
            ] {
                let nested = json!({code:{"httpStatusCode":status}});
                assert!(
                    !error("provider timed out", nested.clone(), None).is_transient(),
                    "{nested}"
                );
                let direct = json!({code:{}, "httpStatusCode":status});
                assert!(
                    !error("provider timed out", direct.clone(), None).is_transient(),
                    "{direct}"
                );
            }
        }
    }

    #[test]
    fn malformed_variant_payload_never_opts_into_retry() {
        for payload in [
            json!("unauthorized"),
            json!(null),
            json!(true),
            json!(503),
            json!([]),
            json!({"unexpected":"overloaded"}),
        ] {
            let info = json!({"responseStreamDisconnected":payload});
            assert!(
                !error("provider timed out", info.clone(), None).is_transient(),
                "{info}"
            );
        }
        assert!(!error(
            "provider timed out",
            json!({"responseStreamDisconnected":{}, "unexpected":true}),
            None
        )
        .is_transient());
    }

    #[test]
    fn nullable_and_valid_integer_status_shapes_keep_documented_retry_behavior() {
        for status in [json!(null), json!(408), json!(500), json!(599)] {
            for code in [
                "responseStreamDisconnected",
                "responseStreamConnectionFailed",
                "responseTooManyFailedAttempts",
                "internalServerError",
            ] {
                let info = json!({code:{"httpStatusCode":status}});
                assert!(
                    error("provider failed", info.clone(), None).is_transient(),
                    "{info}"
                );
            }
        }
    }

    #[test]
    fn quota_auth_and_rate_limit_hints_veto_transient_codes_and_text() {
        for hint in [
            "usage limit reached",
            "quota exhausted",
            "rate_limit_exceeded",
            "too many requests",
            "Unauthorized",
            "authentication failed",
            "invalid API key",
            "token expired",
            "permission denied",
            "insufficient credits",
            "HTTP 429",
            "HTTP status 401",
        ] {
            assert!(
                !error(hint, json!("responseStreamDisconnected"), None).is_transient(),
                "{hint}"
            );
            assert!(
                !error(
                    "provider timed out",
                    json!("internalServerError"),
                    Some(hint)
                )
                .is_transient(),
                "{hint}"
            );
        }
    }

    #[test]
    fn fallback_requires_explicit_timeout_or_overload_and_no_known_code() {
        for message in [
            "request timed out",
            "upstream timeout",
            "provider is overloaded",
            "server temporarily unavailable",
        ] {
            assert!(
                error(message, Value::Null, None).is_transient(),
                "{message}"
            );
        }
        for message in [
            "failed",
            "connection rejected",
            "provider error",
            "time budget exhausted",
        ] {
            assert!(
                !error(message, Value::Null, None).is_transient(),
                "{message}"
            );
        }
    }

    #[test]
    fn revision_context_window_refusals_veto_stream_retry_in_message_and_details() {
        for hint in [
            "context_length_exceeded",
            "Your input exceeds the context window of this model",
            "maximum context length exceeded",
            "CONTEXT-WINDOW-EXCEEDED",
            "contextWindowExceeded",
            "contextLengthExceeded",
            "maximum CONTEXT\n  LENGTH exceeded",
        ] {
            let code = json!({"responseStreamDisconnected":{"httpStatusCode":503}});
            assert!(
                !error(&format!("request timed out: {hint}"), code.clone(), None).is_transient(),
                "message: {hint}"
            );
            assert!(
                !error("stream disconnected before completion", code, Some(hint)).is_transient(),
                "details: {hint}"
            );
            assert!(
                !error("request timed out", Value::Null, Some(hint)).is_transient(),
                "fallback: {hint}"
            );
        }
    }

    #[test]
    fn revision_other_never_retries_even_with_explicit_transient_text_and_status() {
        for info in [
            json!("other"),
            json!({"other":{}}),
            json!({"other":{"httpStatusCode":503}}),
            json!({"other":{},"httpStatusCode":408}),
        ] {
            for message in [
                "request timed out",
                "server overloaded",
                "HTTP 503 temporarily unavailable",
            ] {
                assert!(
                    !error(message, info.clone(), Some("timeout")).is_transient(),
                    "{message}: {info}"
                );
            }
        }
    }

    #[test]
    fn revision_request_bodies_are_removed_before_bounding() {
        let long_body = format!("HTTP 500; request body: {{\"input\":\"confidential-request\",\"padding\":\"{}\",\"customer\":\"private-customer\"}}", "x".repeat(2048));
        for message in [
            "HTTP 500; request body: {\"input\":\"confidential-request\",\"metadata\":{\"customer\":\"private-customer\"}}".to_string(),
            "HTTP 500; REQUEST BODY = [\"confidential-request\"]".to_string(),
            "HTTP 500; request_body=\"confidential-request\"".to_string(),
            "HTTP 500; request-body: confidential-request".to_string(),
            "HTTP 500; requestBody: 'confidential-request'".to_string(),
            "HTTP 500; Request\r\n  Body:\n{\"input\":\"confidential-request\"}".to_string(),
            "HTTP 500; {\"request\":{\"method\":\"POST\",\"body\":{\"input\":\"confidential-request\"}}}".to_string(),
            "HTTP 500; body={\"input\":\"confidential-request\"}".to_string(),
            "HTTP 500; REQUEST_BODY: {\"input\":\"confidential-request".to_string(),
            long_body,
        ] {
            let safe = sanitize_error(&message);
            assert!(safe.starts_with("HTTP 500"), "{safe}");
            assert!(safe.contains("[REDACTED]"), "{safe}");
            assert!(!safe.contains("confidential-request") && !safe.contains("private-customer"), "{safe}");
            assert!(!safe.contains("\"input\"") && !safe.contains("\"metadata\""), "{safe}");
            assert!(safe.len() <= 1024 && !safe.chars().any(char::is_control), "{safe}");
        }
    }

    #[test]
    fn review621_request_payload_is_not_logged() {
        let error: NotificationError = serde_json::from_value(json!({
            "message": "HTTP 500; request payload: {\"input\":\"confidential-request\",\"metadata\":{\"customer\":\"private-customer\"}}",
            "codexErrorInfo": "internalServerError"
        })).unwrap();
        assert!(
            !error.safe_message().contains("confidential-request"),
            "request payload retained: {}",
            error.safe_message()
        );
    }

    #[test]
    fn review621_flat_request_body_is_not_logged() {
        let error: NotificationError = serde_json::from_value(json!({
            "message": "HTTP 500; {\"model\":\"gpt-6\",\"input\":\"confidential-request\"}",
            "codexErrorInfo": "internalServerError"
        }))
        .unwrap();
        assert!(
            !error.safe_message().contains("confidential-request"),
            "flat request body retained: {}",
            error.safe_message()
        );
    }

    #[test]
    fn revision_621_labelled_structured_payloads_are_removed() {
        for message in [
            r#"HTTP 500; payload: {"input":"confidential-request","customer":"private-customer"}"#,
            r#"HTTP 500; PAYLOAD = ["confidential-request", "private-customer"]"#,
            r#"HTTP 500; request_payload={"input":"confidential-request"}"#,
            r#"HTTP 500; request-payload: {"input":"confidential-request"}"#,
            r#"HTTP 500; requestPayload: 'confidential-request'"#,
            "HTTP 500; Request\r\n Payload:\n{\"input\":\"confidential-request\"}",
            r#"HTTP 500; request payload {"input":"confidential-request"}"#,
            r#"HTTP 500; request data: {"input":"confidential-request"}"#,
            r#"HTTP 500; requestJSON="{\"input\":\"confidential-request\"}""#,
            r#"HTTP 500; data={"input":"confidential-request"}"#,
            r#"HTTP 500; input: ["confidential-request"]"#,
            r#"HTTP 500; messages: [{"content":"confidential-request"}]"#,
            r#"HTTP 500; parameters: {"customer":"private-customer"}"#,
            r#"HTTP 500; arguments={'input':'confidential-request'}"#,
            r#"HTTP 500; diagnostic: {"request":{"input":"confidential-request"}}"#,
            r#"HTTP 500; {"envelope":{"input":"confidential-request"}}"#,
            "HTTP 500; envelope: '\n [\"confidential-request\"]'",
            r#"HTTP 500; opaqueDetails=`{"input":"confidential-request"}`"#,
        ] {
            let safe = sanitize_error(message);
            assert!(safe.starts_with("HTTP 500"), "{safe}");
            assert!(safe.contains("[REDACTED]"), "{safe}");
            assert!(
                !safe.contains("confidential-request") && !safe.contains("private-customer"),
                "{safe}"
            );
            assert!(
                !safe.contains("\"input\"") && !safe.contains("\"content\""),
                "{safe}"
            );
            assert!(
                safe.len() <= 1024 && !safe.chars().any(char::is_control),
                "{safe}"
            );
        }
    }

    #[test]
    fn revision_621_labelled_payload_redaction_precedes_bounding() {
        for label in [
            "request payload",
            "serialized request",
            "arguments",
            "event data",
        ] {
            let message = format!(
                "HTTP 500; {label}: {{\"input\":\"confidential-request\",\"padding\":\"{}\",\"customer\":\"private-customer\"}} trailing confidential-request",
                "é".repeat(2048)
            );
            let safe = sanitize_error(&message);
            assert_eq!(safe, format!("HTTP 500; {label} [REDACTED]"));
            assert!(safe.len() <= 1024);
        }
    }

    #[test]
    fn revision_621_key_value_diagnostics_stop_at_the_human_prefix() {
        let diagnostic =
            "HTTP 503; status: 503; retryable=true; request timed out; context: upstream refused";
        assert_eq!(sanitize_error(diagnostic), "HTTP 503; status [REDACTED]");
        let human_message = "HTTP 503 upstream timeout after 30 seconds";
        assert_eq!(sanitize_error(human_message), human_message);
    }

    #[test]
    fn round4_621_only_human_prefix_preceding_dump_delimiters_is_logged() {
        for delimiter in ['{', '[', '<', '=', ':'] {
            for prefix in [
                "HTTP 500 timed out",
                "HTTP 500 timed out\r\n",
                "échec HTTP 500",
            ] {
                let message = format!(
                    "{prefix}{delimiter}\"confidential-request\" customer private-customer"
                );
                let safe = sanitize_error(&message);
                assert_eq!(safe, format!("{} [REDACTED]", prefix.trim_end()));
                assert_eq!(sanitize_error(&safe), safe);
            }
        }
        for message in [
            r#"{"model":"gpt-6","input":"confidential-request"}"#,
            r#"["confidential-request"]"#,
            "<input>confidential-request</input>",
            "=confidential-request",
            ":confidential-request",
        ] {
            assert_eq!(sanitize_error(message), "[REDACTED]");
        }
    }

    #[test]
    fn round4_621_plain_colon_and_equals_dumps_never_pass() {
        for message in [
            "HTTP 500; body: confidential-request customer private-customer",
            "HTTP 500; request values=confidential-request customer private-customer",
            "HTTP 500; unknown: confidential-request",
            "HTTP 500; UNFAMILIAR_LABEL=confidential-request",
            "HTTP 500; request data:\nmodel gpt-6 input confidential-request",
        ] {
            let safe = sanitize_error(message);
            assert!(safe.starts_with("HTTP 500;"), "{safe}");
            assert!(
                !safe.contains("confidential-request") && !safe.contains("private-customer"),
                "{safe}"
            );
            assert!(!safe.contains([':', '=']), "{safe}");
        }
    }

    #[test]
    fn round4_621_allowlisted_prefix_is_bounded_and_utf8_safe() {
        for length in [0, 500, 509, 512, 600] {
            for delimiter in ['{', '[', '<', '=', ':'] {
                let message = format!(
                    "{}{delimiter}confidential-request{}private-customer",
                    "é".repeat(length),
                    "x".repeat(2048)
                );
                let safe = sanitize_error(&message);
                assert!(safe.len() <= 1024, "{}", safe.len());
                assert!(
                    !safe.contains("confidential-request") && !safe.contains("private-customer"),
                    "{safe}"
                );
                assert_eq!(sanitize_error(&safe), safe);
            }
        }
        let long_human_message = format!("{}a {}", "é".repeat(511), "safe".repeat(300));
        let safe = sanitize_error(&long_human_message);
        assert!(safe.len() <= 1024);
        assert_eq!(sanitize_error(&safe), safe);
    }

    #[test]
    fn round4_621_fixed_redaction_markers_are_idempotent_and_hide_the_tail() {
        for message in [
            "HTTP 500 [REDACTED] confidential-request",
            "[REDACTED] confidential-request",
            "HTTP 500 Bearer fixture-secret confidential-request",
            "HTTP 500 password 'fixture-secret' confidential-request",
            "HTTP 500 Authorization Bearer fixture-secret confidential-request",
        ] {
            let safe = sanitize_error(message);
            assert!(safe.contains("[REDACTED]"), "{safe}");
            assert!(
                !safe.contains("fixture-secret") && !safe.contains("confidential-request"),
                "{safe}"
            );
            assert_eq!(sanitize_error(&safe), safe);
        }
    }

    #[test]
    fn revision_api_key_headers_are_removed_before_bounding() {
        let long_key = format!(
            "HTTP 500; X-API-KEY: \"opaque-fixture-secret{}private-customer\"",
            "x".repeat(2048)
        );
        for message in [
            "HTTP 500; x-api-key: opaque-fixture-secret".to_string(),
            "HTTP 500; X-API-KEY = 'opaque-fixture-secret'".to_string(),
            "HTTP 500; {\"X-API-KEY\":\"opaque-fixture-secret\"}".to_string(),
            "HTTP 500;\nx-Api-Key:\n\"opaque-fixture-secret\"".to_string(),
            "HTTP 500; X_API_KEY: opaque-fixture-secret".to_string(),
            long_key,
        ] {
            let safe = sanitize_error(&message);
            assert!(safe.starts_with("HTTP 500"), "{safe}");
            assert!(safe.contains("[REDACTED]"), "{safe}");
            assert!(
                !safe.contains("opaque-fixture-secret") && !safe.contains("private-customer"),
                "{safe}"
            );
            assert!(
                safe.len() <= 1024 && !safe.chars().any(char::is_control),
                "{safe}"
            );
        }
    }

    #[test]
    fn safe_code_exposes_only_allowlisted_variant_and_numeric_status() {
        let unknown = error(
            "failed",
            json!({"private-secret-value":{"httpStatusCode":502,"token":"private-value"}}),
            None,
        );
        assert_eq!(unknown.safe_code(), None);
        assert_eq!(unknown.safe_http_status(), Some(502));
        assert_eq!(
            error(
                "failed",
                json!({"badRequest":{"secret":"private-value"}}),
                None
            )
            .safe_code()
            .as_deref(),
            Some("badRequest")
        );
        assert_eq!(
            error("failed", json!({"httpStatusCode":"private-value"}), None).safe_http_status(),
            None
        );
    }

    #[test]
    fn redacts_credentials_and_retains_useful_error_words() {
        let api_key = format!("sk-{}", "x".repeat(48));
        let jwt = format!(
            "eyJ{}.eyJ{}.{}",
            "a".repeat(24),
            "b".repeat(24),
            "c".repeat(32)
        );
        for message in [
            format!("Provider timed out with key {api_key}"),
            "Provider timed out; Authorization: Bearer private-secret-value".to_string(),
            "Provider timed out; Bearer private-secret-value".to_string(),
            "Provider timed out; api_key=private-secret-value".to_string(),
            "Provider timed out; key=private-secret-value".to_string(),
            "Provider timed out; accessToken=private-secret-value".to_string(),
            "Provider timed out; access_token: private-secret-value".to_string(),
            "Provider timed out; password='private secret value'".to_string(),
            "Provider timed out; password=\"private \\\"secret\\\" value\"".to_string(),
            "Provider timed out; {\"token\":\"private secret value\"}".to_string(),
            format!("Provider timed out: {jwt}"),
            format!("Provider timed out: https://example.invalid/{api_key}"),
        ] {
            let safe = error(&message, Value::Null, None).safe_message();
            assert!(safe.contains("Provider timed out"), "{safe}");
            assert!(safe.contains("[REDACTED]"), "{safe}");
            assert!(!safe.contains("private"), "{safe}");
            assert!(!safe.contains(&api_key), "{safe}");
            assert!(!safe.contains(&jwt), "{safe}");
        }
    }

    #[test]
    fn diagnostic_text_is_bounded_after_redaction_and_control_cleanup() {
        let key = format!("sk-{}", "x".repeat(1500));
        let safe = sanitize_error(&format!(
            "Provider\nfailed\u{1b}: {key} {}",
            "é".repeat(1500)
        ));
        assert!(safe.len() <= 1024, "{}", safe.len());
        assert!(!safe.chars().any(char::is_control));
        assert!(safe.contains("[REDACTED]"));
        assert!(!safe.contains("sk-"));
    }
}
