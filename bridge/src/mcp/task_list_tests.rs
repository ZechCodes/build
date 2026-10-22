use super::*;

fn call(arguments: Value) -> Handled {
    DoneServer::new("task-page-test").handle_message(
        &json!({
            "jsonrpc":"2.0","id":1,"method":"tools/call",
            "params":{"name":"list_tasks","arguments":arguments}
        })
        .to_string(),
    )
}

#[test]
fn task_list_tool_describes_bounded_cursor_pages_and_forwards_arguments() {
    let tools = DoneServer::new("task-page-test").tools();
    let tool = tools
        .as_array()
        .unwrap()
        .iter()
        .find(|tool| tool["name"] == "list_tasks")
        .unwrap();
    assert_eq!(tool["inputSchema"]["properties"]["limit"]["maximum"], 20);
    assert_eq!(tool["inputSchema"]["properties"]["limit"]["default"], 20);
    let description = tool["description"].as_str().unwrap();
    assert!(description.contains("next_cursor") && description.contains("empty filtered page"));
    let action = call(json!({"limit":7,"cursor":"opaque-cursor","state":"open","label":"Bug"}))
        .action
        .unwrap();
    let value = serde_json::to_value(action).unwrap();
    assert_eq!(value["limit"], 7);
    assert_eq!(value["cursor"], "opaque-cursor");
    assert_eq!(value["label"], "Bug");
    assert_eq!(value["state"], "open");
}

#[test]
fn task_list_tool_refuses_invalid_limits_and_cursor_types() {
    for limit in [json!(0), json!(21), json!(-1), json!(1.5), json!("20")] {
        let response = call(json!({"limit":limit}));
        assert!(response.action.is_none(), "{limit}");
        assert!(response.reply.is_some());
    }
    assert!(call(json!({"cursor":42})).action.is_none());
    assert!(call(json!({})).action.is_some());
}
