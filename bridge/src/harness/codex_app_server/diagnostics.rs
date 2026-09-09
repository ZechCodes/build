use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

const PREFIX: &str = "build_lifecycle ";

#[derive(Clone)]
pub struct SessionDiagnostics {
    harness_session_id: String,
    #[cfg(test)]
    captured: std::sync::Arc<std::sync::Mutex<Vec<Value>>>,
}

impl SessionDiagnostics {
    pub fn new() -> SessionDiagnostics {
        SessionDiagnostics {
            harness_session_id: uuid::Uuid::new_v4().to_string(),
            #[cfg(test)]
            captured: Default::default(),
        }
    }

    pub fn emit<'a>(
        &self,
        event: &str,
        elapsed: Duration,
        fields: impl IntoIterator<Item = (&'a str, Value)>,
    ) {
        let line = self.render(event, elapsed, fields);
        #[cfg(test)]
        self.captured.lock().unwrap().push(
            serde_json::from_str(line.strip_prefix(PREFIX).expect("diagnostic prefix"))
                .expect("diagnostic JSON"),
        );
        eprintln!("{line}");
    }

    fn render<'a>(
        &self,
        event: &str,
        elapsed: Duration,
        fields: impl IntoIterator<Item = (&'a str, Value)>,
    ) -> String {
        let mut object = Map::new();
        object.insert("component".into(), Value::String("codex_app_server".into()));
        object.insert("event".into(), Value::String(event.into()));
        object.insert("ts_utc".into(), Value::String(utc_now()));
        object.insert("ts_unix_ms".into(), Value::from(unix_millis()));
        object.insert(
            "harness_session_id".into(),
            Value::String(self.harness_session_id.clone()),
        );
        object.insert("elapsed_ms".into(), Value::from(elapsed.as_millis() as u64));
        for (key, value) in fields {
            object.insert(key.into(), value);
        }
        format!("{PREFIX}{}", Value::Object(object))
    }

    #[cfg(test)]
    pub fn captured(&self) -> Vec<Value> {
        self.captured.lock().unwrap().clone()
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn utc_now() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_line_has_correlation_and_no_payload_fields() {
        let diagnostics = SessionDiagnostics::new();
        let line = diagnostics.render(
            "turn_outgoing",
            Duration::from_millis(7),
            [("method", Value::String("turn/start".into()))],
        );
        let object: Value = serde_json::from_str(line.strip_prefix(PREFIX).unwrap()).unwrap();
        assert_eq!(object["component"], "codex_app_server");
        assert_eq!(object["elapsed_ms"], 7);
        assert!(object["harness_session_id"].as_str().unwrap().len() > 20);
        for forbidden in ["prompt", "params", "frame", "stderr"] {
            assert!(object.get(forbidden).is_none());
        }
    }
}
