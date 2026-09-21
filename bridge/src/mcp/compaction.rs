//! The two tools that compact a session: `compact_self`, on every surface
//! whose agent has one, and `compact_agent`, on the project agent's alone.
//!
//! Both only parse here. Whether the session is compacted now or when its turn
//! ends, and whether it can be at all, is the daemon's to decide: it holds the
//! sessions and knows which of them is working.

use super::{acted, refused, required_argument, BridgeAction, Handled};
use serde_json::{json, Value};

/// What `compact_self` says about itself on every `tools/list`.
const COMPACT_SELF_DESCRIPTION: &str = "Compact your own session when this turn ends: Build summarizes your context so the next piece of work starts small, keeping what the instructions name. Nothing is interrupted; the compaction runs after your current turn and before your next one. Codex compacts without a focus, so on Codex the instructions are ignored.";

/// What `compact_agent` says about itself on every `tools/list`.
const COMPACT_AGENT_DESCRIPTION: &str = "Compact another agent's session, keeping what the instructions name. Between its turns it compacts at once; mid-turn it is never interrupted, and compacts when its current turn ends. An agent with no running session, or whose harness cannot compact, is refused. Codex compacts without a focus, so for a Codex agent the instructions are ignored.";

/// What the `instructions` argument is for, the same on both tools.
const INSTRUCTIONS_DESCRIPTION: &str = "The compaction's focus: what the summary should keep for the next piece of work. Ignored on Codex.";

pub(super) fn compact_self_tool() -> Value {
    json!({
        "name": "compact_self",
        "description": COMPACT_SELF_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "instructions": { "type": "string", "description": INSTRUCTIONS_DESCRIPTION }
            },
            "required": ["instructions"]
        }
    })
}

pub(super) fn compact_agent_tool() -> Value {
    json!({
        "name": "compact_agent",
        "description": COMPACT_AGENT_DESCRIPTION,
        "inputSchema": {
            "type": "object",
            "properties": {
                "agent_id": { "type": "string", "description": "From list_workspace_agents." },
                "instructions": { "type": "string", "description": INSTRUCTIONS_DESCRIPTION }
            },
            "required": ["agent_id", "instructions"]
        }
    })
}

pub(super) fn compact_self_action(id: Value, params: Option<&Value>) -> Handled {
    match required_argument(params, "instructions") {
        Ok(instructions) => acted(id, BridgeAction::CompactSelf { instructions }),
        Err(message) => refused(id, message),
    }
}

pub(super) fn compact_agent_action(id: Value, params: Option<&Value>) -> Handled {
    match required_argument(params, "agent_id")
        .and_then(|agent_id| Ok((agent_id, required_argument(params, "instructions")?)))
    {
        Ok((agent_id, instructions)) => acted(
            id,
            BridgeAction::CompactAgent {
                agent_id,
                instructions,
            },
        ),
        Err(message) => refused(id, message),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{BridgeAction, DoneServer};
    use serde_json::Value;

    fn listed(owner: &str) -> Vec<Value> {
        let listed = DoneServer::for_owner(owner)
            .handle_message(r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#);
        let reply: Value = serde_json::from_str(&listed.reply.unwrap()).unwrap();
        reply["result"]["tools"].as_array().unwrap().clone()
    }

    fn named(owner: &str, name: &str) -> Option<Value> {
        listed(owner).into_iter().find(|tool| tool["name"] == name)
    }

    fn call(owner: &str, name: &str, arguments: &str) -> Option<BridgeAction> {
        DoneServer::for_owner(owner)
            .handle_message(&format!(
                r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
            ))
            .action
    }

    /// Every agent can compact itself; only the project agent compacts
    /// another, because only it staffs the project's workspaces.
    #[test]
    fn compact_agent_is_the_project_agents_and_compact_self_everyones() {
        assert!(named("project-01H", "compact_agent").is_some());
        assert!(named("agent-01H", "compact_agent").is_none());
        assert!(named("router-abc", "compact_agent").is_none());
        assert!(named("project-01H", "compact_self").is_some());
        assert!(named("agent-01H", "compact_self").is_some());
    }

    /// The instructions are the focus of the summary, and the description says
    /// so, and that Codex compacts without one.
    #[test]
    fn the_descriptions_say_what_the_instructions_are_for_and_that_codex_ignores_them() {
        for (owner, name) in [
            ("project-01H", "compact_agent"),
            ("agent-01H", "compact_self"),
        ] {
            let tool = named(owner, name).unwrap();
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains("Codex"), "{name}: {description}");
            let instructions = tool["inputSchema"]["properties"]["instructions"]["description"]
                .as_str()
                .unwrap();
            assert!(instructions.contains("keep"), "{name}: {instructions}");
            assert!(tool["inputSchema"]["required"]
                .as_array()
                .unwrap()
                .contains(&Value::from("instructions")));
        }
    }

    #[test]
    fn each_call_emits_its_typed_action() {
        assert_eq!(
            call(
                "project-01H",
                "compact_agent",
                r#"{"agent_id":"agent-7","instructions":" keep the API notes "}"#
            ),
            Some(BridgeAction::CompactAgent {
                agent_id: "agent-7".into(),
                instructions: "keep the API notes".into(),
            })
        );
        assert_eq!(
            call(
                "agent-01H",
                "compact_self",
                r#"{"instructions":"the rail"}"#
            ),
            Some(BridgeAction::CompactSelf {
                instructions: "the rail".into(),
            })
        );
        assert_eq!(
            call("project-01H", "compact_agent", r#"{"agent_id":"a"}"#),
            None
        );
        assert_eq!(
            call(
                "agent-01H",
                "compact_agent",
                r#"{"agent_id":"a","instructions":"x"}"#
            ),
            None
        );
    }

    #[test]
    fn each_action_names_its_tool_and_its_surfaces() {
        use super::super::McpSurface;
        let agent = BridgeAction::CompactAgent {
            agent_id: "a".into(),
            instructions: "x".into(),
        };
        let own = BridgeAction::CompactSelf {
            instructions: "x".into(),
        };
        assert_eq!(agent.tool_name(), "compact_agent");
        assert_eq!(own.tool_name(), "compact_self");
        assert!(agent.allowed_on(McpSurface::Project));
        assert!(!agent.allowed_on(McpSurface::Coding));
        assert!(own.allowed_on(McpSurface::Coding));
        assert!(own.allowed_on(McpSurface::Project));
        assert!(!own.allowed_on(McpSurface::Router));
    }
}
