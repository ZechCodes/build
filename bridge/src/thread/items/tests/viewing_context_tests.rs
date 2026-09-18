use super::super::*;

#[test]
fn viewing_context_accepts_the_versioned_wire_shape() {
    let context: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [
            { "kind": "file", "path": "src/lib.rs" },
            { "kind": "commit", "sha": "0123456789abcdef0123456789abcdef01234567" },
            { "kind": "diff", "path": "src/app.rs", "mode": "uncommitted" },
            { "kind": "selection", "path": "src/app.rs", "text": "selected", "line_start": 4, "line_end": 5, "side": "new", "unsaved": true, "truncated": true }
        ]
    })).unwrap();

    assert_eq!(context.validate(), Ok(()));
    assert_eq!(context.items.len(), 4);
}

#[test]
fn viewing_context_rejects_unsafe_paths_and_partial_shas() {
    let escaping: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [{ "kind": "file", "path": "../secret" }]
    }))
    .unwrap();
    assert!(escaping.validate().unwrap_err().contains("path"));

    let partial: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [{ "kind": "commit", "sha": "0123456" }]
    }))
    .unwrap();
    assert!(partial.validate().unwrap_err().contains("sha"));
}

#[test]
fn viewing_context_enforces_item_and_excerpt_budgets() {
    let item = ViewingContextItem::File {
        path: "src/lib.rs".into(),
    };
    let too_many = ViewingContext {
        version: 1,
        items: vec![item; 101],
    };
    assert!(too_many.validate().unwrap_err().contains("100"));

    let oversized = ViewingContext {
        version: 1,
        items: vec![ViewingContextItem::Selection {
            path: "src/lib.rs".into(),
            text: "x".repeat(32 * 1024 + 1),
            line_start: None,
            line_end: None,
            side: None,
            unsaved: false,
            truncated: false,
        }],
    };
    assert!(oversized.validate().unwrap_err().contains("32768"));
}

#[test]
fn selection_line_end_requires_a_start() {
    let context = ViewingContext {
        version: 1,
        items: vec![ViewingContextItem::Selection {
            path: "src/lib.rs".into(),
            text: "chosen".into(),
            line_start: None,
            line_end: Some(4),
            side: None,
            unsaved: false,
            truncated: false,
        }],
    };
    assert!(context.validate().unwrap_err().contains("line range"));
}

/// Forward compatibility: `viewing_context` rides v1 request paths
/// (`thread.post`, `run.message`, `plan.message`, `issue.send_notes`), and a
/// v1 request never refuses a field this bridge predates — a newer SPA that
/// names something here must still be able to talk to an older bridge.
#[test]
fn viewing_context_accepts_a_field_this_bridge_predates() {
    let context: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "focus": "editor",
        "items": [{ "kind": "file", "path": "src/lib.rs" }]
    }))
    .expect("an unknown top-level field is ignored");

    assert_eq!(context.validate(), Ok(()));
    assert_eq!(
        context.items,
        vec![ViewingContextItem::File {
            path: "src/lib.rs".into()
        }],
        "the known fields survive the unknown one"
    );
}

#[test]
fn a_viewing_context_item_accepts_a_field_this_bridge_predates() {
    let context: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [
            { "kind": "file", "path": "src/lib.rs", "pinned": true },
            {
                "kind": "selection",
                "path": "src/app.rs",
                "text": "selected",
                "line_start": 4,
                "line_end": 5,
                "collapsed": false
            }
        ]
    }))
    .expect("an unknown field inside an item is ignored");

    assert_eq!(context.validate(), Ok(()));
    assert_eq!(
        context.items,
        vec![
            ViewingContextItem::File {
                path: "src/lib.rs".into()
            },
            ViewingContextItem::Selection {
                path: "src/app.rs".into(),
                text: "selected".into(),
                line_start: Some(4),
                line_end: Some(5),
                side: None,
                unsaved: false,
                truncated: false,
            },
        ],
        "the known fields survive the unknown ones"
    );
}

/// The rail carries the project's conversation into every workspace, so a
/// message sent from one says which workspace it was sent from. Additive at
/// version 1: an older SPA sends no such item and reads past one it is handed.
#[test]
fn viewing_context_carries_the_workspace_a_message_was_sent_from() {
    let context: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [
            { "kind": "workspace", "workspace_id": "ws-3f2a91c4", "name": "wire-facade" },
            { "kind": "file", "path": "src/lib.rs" }
        ]
    }))
    .unwrap();

    assert_eq!(context.validate(), Ok(()));
    assert_eq!(
        context.items.first(),
        Some(&ViewingContextItem::Workspace {
            workspace_id: "ws-3f2a91c4".into(),
            name: "wire-facade".into(),
        })
    );
    assert_eq!(
        serde_json::to_value(&context.items[0]).unwrap(),
        serde_json::json!({ "kind": "workspace", "workspace_id": "ws-3f2a91c4", "name": "wire-facade" }),
        "the item goes back out exactly as it came in"
    );
}

#[test]
fn viewing_context_rejects_a_workspace_item_naming_nothing() {
    let nameless: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [{ "kind": "workspace", "workspace_id": "ws-1", "name": "" }]
    }))
    .unwrap();
    assert!(nameless.validate().unwrap_err().contains("workspace"));

    let unaddressed: ViewingContext = serde_json::from_value(serde_json::json!({
        "version": 1,
        "items": [{ "kind": "workspace", "workspace_id": "", "name": "wire-facade" }]
    }))
    .unwrap();
    assert!(unaddressed.validate().unwrap_err().contains("workspace"));
}
