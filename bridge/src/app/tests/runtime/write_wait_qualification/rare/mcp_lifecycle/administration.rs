use super::*;

pub(super) async fn sources(context: &mut ProjectFixture) {
    let assets = context.root.join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(assets.join("sentinel.txt"), "original source bytes\n").unwrap();
    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::AddProjectSource {
                path: Some(assets.display().to_string()),
                remote: None,
                name: Some("assets".into()),
                base_branch: None,
            },
        )
        .await;
    assert_eq!(proof.reply["result"]["project_id"], context.project);
    let source = proof.reply["result"]["sources"][1]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let row = project_row(context);
    assert_eq!(row["sources"].as_array().unwrap().len(), 2);
    assert_eq!(row["sources"][1]["is_git"], false);
    let config = disk_json(&context.root.join("config.json"));
    let saved = config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == context.project)
        .unwrap();
    assert_eq!(saved["sources"][1]["id"], source);
    context.fixture.verify(proof, json!({"caller":context.caller.agent,"project":context.project,"persisted_source":source,"source_path":assets})).await;

    let proof = context
        .fixture
        .call_mcp(
            &context.caller.agent,
            &context.caller.token,
            BridgeAction::RemoveProjectSource {
                source_id: source.clone(),
            },
        )
        .await;
    assert_eq!(project_row(context)["sources"].as_array().unwrap().len(), 1);
    let config = disk_json(&context.root.join("config.json"));
    let saved = config["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == context.project)
        .unwrap();
    assert_eq!(saved["sources"].as_array().unwrap().len(), 1);
    assert_eq!(
        std::fs::read_to_string(assets.join("sentinel.txt")).unwrap(),
        "original source bytes\n"
    );
    context.fixture.verify(proof, json!({"caller":context.caller.agent,"removed_source":source,"source_preserved":assets})).await;
}
