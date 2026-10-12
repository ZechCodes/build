//! A project worker carries the project prefix but works on the coding surface.
//! Every harness's launch declares that to Build's MCP server, so the tools the
//! session lists are the ones the daemon will accept from it; every other
//! launch declares nothing and is left exactly as it was.

use crate::harness::{harness_for, HarnessContext, SpawnOptions};
use crate::mcp::{DoneServer, McpSurface, MCP_SURFACE_ENV};
use crate::models::{AgentProvider, ModelChoice};
use crate::pty::HarnessSpec;

fn spec_for(provider: AgentProvider, owner_id: &str, granted: McpSurface) -> HarnessSpec {
    let worktree = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let options = SpawnOptions {
        owner_id: owner_id.to_string(),
        mcp_session_token: "token".to_string(),
        cwd: worktree.path().to_path_buf(),
        mcp_surface: Some(granted),
        ..SpawnOptions::default()
    };
    let context = HarnessContext {
        bridge_exe: "/usr/local/bin/build-bridge".into(),
        mcp_socket: state.path().join("build-mcp.sock"),
        state_root: state.path().to_path_buf(),
    };
    let choice = ModelChoice {
        provider,
        ..ModelChoice::default()
    };
    harness_for(provider)
        .spec(&choice, &options, &context)
        .unwrap_or_else(|error| panic!("{provider:?}: {error}"))
}

/// What the launch tells its MCP server about the surface: through the
/// environment the server inherits, or (codex) through the server's own
/// configured environment.
fn declared_surface(spec: &HarnessSpec) -> Option<String> {
    let inherited = spec
        .env
        .iter()
        .find(|(key, _)| key == MCP_SURFACE_ENV)
        .map(|(_, value)| value.clone());
    let configured_prefix = format!("mcp_servers.build.env.{MCP_SURFACE_ENV}=");
    let configured = spec.args.iter().find_map(|arg| {
        arg.strip_prefix(&configured_prefix)
            .map(|value| serde_json::from_str::<String>(value).unwrap())
    });
    inherited.or(configured)
}

#[test]
fn every_harness_declares_a_project_workers_coding_surface() {
    for provider in AgentProvider::ALL {
        let worker = spec_for(provider, "project-01WORKER", McpSurface::Coding);
        assert_eq!(
            declared_surface(&worker).as_deref(),
            Some("coding"),
            "{provider:?}"
        );
    }
}

#[test]
fn no_other_launch_declares_a_surface() {
    for provider in AgentProvider::ALL {
        for (owner, granted) in [
            ("project-01PIN", McpSurface::Project),
            ("agent-01CODING", McpSurface::Coding),
        ] {
            let spec = spec_for(provider, owner, granted);
            assert_eq!(declared_surface(&spec), None, "{provider:?} {owner}");
        }
    }
}

/// Codex never asks `tools/list`: its allow-list is the launch's, so a worker's
/// is the coding surface's, not the project surface its prefix names.
#[test]
fn codex_allows_a_project_worker_the_coding_tools() {
    let coding = serde_json::to_string(&DoneServer::tool_names_of(McpSurface::Coding)).unwrap();
    for provider in [AgentProvider::Codex, AgentProvider::CodexAppServer] {
        let spec = spec_for(provider, "project-01WORKER", McpSurface::Coding);
        let allowed = spec
            .args
            .iter()
            .find_map(|arg| arg.strip_prefix("mcp_servers.build.enabled_tools="))
            .unwrap_or_else(|| panic!("{provider:?}: {:?}", spec.args));
        assert_eq!(allowed, coding, "{provider:?}");
    }
}

/// The server a launch starts lists what its declaration narrows it to, and a
/// declaration can never widen: no copied value hands an id the project or
/// router tools.
#[test]
fn a_launch_declaration_only_narrows_the_listed_surface() {
    assert_eq!(
        DoneServer::launched("project-01W", Some("coding")).surface(),
        McpSurface::Coding
    );
    assert_eq!(
        DoneServer::launched("project-01P", None).surface(),
        McpSurface::Project
    );
    for forged in ["project", "router", "Project", ""] {
        assert_eq!(
            DoneServer::launched("agent-01A", Some(forged)).surface(),
            McpSurface::Coding,
            "{forged}"
        );
        assert_eq!(
            DoneServer::launched("project-01P", Some(forged)).surface(),
            McpSurface::Project,
            "{forged}"
        );
    }
    assert_eq!(
        DoneServer::launched("router-01R", Some("coding")).surface(),
        McpSurface::Router
    );
}
