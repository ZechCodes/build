//! Examples must describe policies the production resolver/adapter can emit,
//! rather than merely round-trip through the permissive observation fields.
use build_bridge::compaction::{CompactionSource, ResolvedCompactionPolicy};
use build_bridge::harness::compaction::{CompactionEnforcement, CompactionLaunchIdentity};
use build_bridge::harness::harness_for;
use build_bridge::models::{AgentProvider, ModelChoice};
use serde_json::{json, Value};
use std::path::Path;

fn visit(value: &Value, check: &mut impl FnMut(&Value)) {
    match value {
        Value::Object(fields) => {
            check(value);
            for value in fields.values() {
                visit(value, check);
            }
        }
        Value::Array(values) => {
            for value in values {
                visit(value, check);
            }
        }
        _ => {}
    }
}

fn fixtures(check: &mut impl FnMut(&Value)) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../fixtures/api/v1");
    for entry in std::fs::read_dir(root).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
            visit(&value, check);
        }
    }
}

fn source(value: &Value) -> CompactionSource {
    let id = || value["id"].as_str().unwrap().to_owned();
    match value["kind"].as_str().unwrap() {
        "default" => CompactionSource::Default,
        "device" => CompactionSource::Device,
        "project" => CompactionSource::Project(id()),
        "workspace" => CompactionSource::Workspace(id()),
        "conversation" => CompactionSource::Conversation(id()),
        kind => panic!("unknown source {kind}"),
    }
}

fn tokens(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| value["tokens_decimal"].as_str().unwrap().parse().unwrap())
}

#[test]
fn compaction_fixture_scope_maps_match_their_effective_observations() {
    let mut count = 0;
    fixtures(&mut |record| {
        let Some(raw) = record
            .get("compaction_thresholds")
            .filter(|_| record["compaction"]["effective"].is_object())
        else {
            return;
        };
        count += 1;
        for provider in AgentProvider::ALL {
            let name = provider.wire_id();
            let effective = &record["compaction"]["effective"][name];
            if raw[name].is_null() {
                assert_eq!(effective, &record["compaction"]["inherited"][name]);
            } else {
                let expected = ResolvedCompactionPolicy {
                    tokens: tokens(&raw[name]),
                    source: if let Some(id) = record["workspace_id"].as_str() {
                        CompactionSource::Workspace(id.into())
                    } else if let Some(id) = record["project_id"].as_str() {
                        CompactionSource::Project(id.into())
                    } else {
                        CompactionSource::Device
                    },
                };
                assert_eq!(*effective, expected.observation());
            }
        }
    });
    assert!(count > 0);
}

#[test]
fn compaction_fixture_agents_match_configured_inheritance() {
    fixtures(&mut |agent| {
        let Some(compaction) = agent
            .get("compaction")
            .filter(|c| c["configured"].is_object())
        else {
            return;
        };
        if let Some(tokens) = agent["max_context_tokens"].as_u64() {
            let expected = ResolvedCompactionPolicy {
                tokens,
                source: CompactionSource::Conversation(
                    agent["conversation_id"].as_str().unwrap().into(),
                ),
            };
            assert_eq!(compaction["configured"], expected.observation());
        } else {
            assert_eq!(
                compaction["inherited"], compaction["configured"],
                "{} has no override",
                agent["id"]
            );
        }
    });
}

#[test]
fn compaction_fixture_agents_match_production_preparation() {
    let mut applied = 0;
    fixtures(&mut |agent| {
        let Some(compaction) = agent
            .get("compaction")
            .filter(|c| c["configured"].is_object())
        else {
            return;
        };
        if compaction["active"].is_null() {
            assert_eq!(compaction["enforcement"], "unsupported");
            assert_eq!(compaction["reason"], "active_policy_unknown");
            assert!(compaction["pending_change"].is_null());
            return;
        }
        applied += 1;
        check_applied(agent, compaction);
    });
    assert!(
        applied > 0,
        "keep an applied policy example in the contract"
    );
}

fn check_applied(agent: &Value, compaction: &Value) {
    let active = &compaction["active"];
    let provider = AgentProvider::from_wire(active["provider"].as_str().unwrap()).unwrap();
    let requested = ResolvedCompactionPolicy {
        tokens: active["tokens"].as_u64().unwrap_or_else(|| tokens(active)),
        source: source(&active["source"]),
    };
    let policy = harness_for(provider)
        .prepare_compaction_policy(
            requested,
            ModelChoice {
                provider,
                model: agent["active_model"].as_str().map(str::to_owned),
                effort: agent["effort"].as_str().map(str::to_owned),
            },
            active["qualification"]["version"]
                .as_str()
                .map(str::to_owned),
            CompactionLaunchIdentity::new("fixture-only-no-session"),
        )
        .unwrap();
    let enforcement = match policy.enforcement {
        CompactionEnforcement::Native => "native",
        CompactionEnforcement::BetweenTurn => "between_turn",
        CompactionEnforcement::Unsupported => "unsupported",
    };
    assert_eq!(compaction["enforcement"], enforcement);
    assert_eq!(compaction["reason"], json!(policy.reason));
    assert_eq!(
        active["qualification"],
        json!({ "version": policy.capabilities.qualification.version, "evidence": policy.capabilities.qualification.evidence })
    );
    assert_eq!(
        active["translated_trigger"],
        json!(policy.translated_trigger)
    );
    assert_eq!(active["evidence"], "applied_at_start");
    let pending = policy.requested.observation() != compaction["configured"]
        || agent["provider"] != active["provider"];
    assert_eq!(compaction["pending_change"], pending);
}
