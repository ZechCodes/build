use super::*;

pub(super) fn check(method: &str, value: &Value, e: &Witnesses) -> bool {
    match method {
        "models.list" => {
            assert!(array(value, "/providers")
                .iter()
                .any(|provider| provider["models"]
                    .as_array()
                    .is_some_and(|models| models.iter().any(|m| m["id"] == e.model_id))));
        }
        "harnesses.list" => {
            let installed = row(value, "/harnesses", "id", &e.installed_harness);
            assert_eq!(installed["installation"]["state"], "installed");
            assert_eq!(installed["version"]["installed"], "2.1.999");
            assert!(installed["installation"]["checked_at_ms"]
                .as_u64()
                .is_some());
            let missing = row(value, "/harnesses", "id", &e.missing_harness);
            assert_eq!(missing["installation"]["state"], "not_installed");
            assert_eq!(value["refresh"]["requested"], value["refresh"]["completed"]);
        }
        "bridge.update_status" => {
            assert_eq!(value["running_version"], "0.2.0");
            assert_eq!(value["platform"], "linux-x86_64");
            assert_eq!(value["state"], "available");
            assert_eq!(value["latest_release"]["tag"], "bridge-v0.3.0");
        }
        _ => return false,
    }
    true
}
