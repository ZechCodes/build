use super::*;
pub(super) fn verify(
    request: &CommonRequest,
    result: &Value,
    actual: SavedEffect,
) -> Result<OracleWitness, String> {
    let mut fields = Vec::new();
    match (&request.effect, actual) {
        (
            ExpectedEffect::Posted {
                owner,
                agent,
                thread,
                body,
                role,
                operation_id,
            },
            SavedEffect::Message {
                owner: actual_owner,
                agent: actual_agent,
                thread: actual_thread,
                item,
                operation_status,
                matched_actual_session_turn,
                mandatory_children_complete,
            },
        ) => {
            verify_message(
                MessageExpectation {
                    address: Address {
                        owner,
                        agent,
                        thread,
                    },
                    body,
                    role,
                    operation: operation_id.as_deref(),
                    from_agent: None,
                },
                Address {
                    owner: &actual_owner,
                    agent: &actual_agent,
                    thread: &actual_thread,
                },
                &item,
            )?;
            if operation_id.is_some() {
                ensure(
                    operation_status.as_deref() == Some("delivered")
                        && matched_actual_session_turn
                            .as_ref()
                            .is_some_and(|turn| turn.contains(body)),
                    "actual SPA post receipt/session turn did not deliver exact body",
                )?;
            }
            ensure(
                mandatory_children_complete,
                "actual post mandatory children remain unfinished",
            )?;
            fields.push(assertion(
                "saved_message_id",
                WitnessScalar::Text(
                    item["data"]["id"]
                        .as_str()
                        .ok_or("saved message ID absent")?
                        .into(),
                ),
            ));
            if let Some(operation) = operation_id {
                fields.push(assertion(
                    "saved_operation_id",
                    WitnessScalar::Text(operation.clone()),
                ));
            }
            fields.push(assertion("saved_body", WitnessScalar::Text(body.clone())));
        }
        (
            ExpectedEffect::Handoff {
                caller,
                recipient_owner,
                recipient,
                thread,
                body,
            },
            SavedEffect::Message {
                owner: actual_owner,
                agent: actual_agent,
                thread: actual_thread,
                item,
                operation_status,
                matched_actual_session_turn,
                mandatory_children_complete,
            },
        ) => {
            let operation = result["operation_id"]
                .as_str()
                .ok_or("actual MCP handoff receipt lacks operation ID")?;
            verify_message(
                MessageExpectation {
                    address: Address {
                        owner: recipient_owner,
                        agent: recipient,
                        thread,
                    },
                    body,
                    role: "user",
                    operation: Some(operation),
                    from_agent: Some(caller),
                },
                Address {
                    owner: &actual_owner,
                    agent: &actual_agent,
                    thread: &actual_thread,
                },
                &item,
            )?;
            ensure(
                operation_status.as_deref() == Some("delivered")
                    && matched_actual_session_turn
                        .as_ref()
                        .is_some_and(|turn| turn.contains(body))
                    && mandatory_children_complete,
                "actual handoff receipt/session turn/mandatory children unfinished",
            )?;
            fields.push(assertion(
                "saved_operation_id",
                WitnessScalar::Text(operation.into()),
            ));
            fields.push(assertion(
                "recipient",
                WitnessScalar::Text(recipient.clone()),
            ));
            fields.push(assertion("saved_body", WitnessScalar::Text(body.clone())));
        }
        _ => return Err("native saved witness type does not match actual request effect".into()),
    }
    Ok(state_witness(request, fields))
}

struct Address<'a> {
    owner: &'a str,
    agent: &'a str,
    thread: &'a str,
}
struct MessageExpectation<'a> {
    address: Address<'a>,
    body: &'a str,
    role: &'a str,
    operation: Option<&'a str>,
    from_agent: Option<&'a String>,
}
fn verify_message(
    expected: MessageExpectation<'_>,
    actual: Address<'_>,
    item: &Value,
) -> Result<(), String> {
    ensure(
        expected.address.owner == actual.owner
            && expected.address.agent == actual.agent
            && expected.address.thread == actual.thread
            && item["type"] == "message"
            && item["data"]["body"].as_str() == Some(expected.body)
            && item["data"]["role"].as_str() == Some(expected.role),
        "actual canonical saved message address/body/role differs",
    )?;
    if let Some(operation) = expected.operation {
        ensure(
            item["data"]["operation_id"].as_str() == Some(operation),
            "saved exact message operation differs",
        )?;
    }
    if let Some(from) = expected.from_agent {
        ensure(
            item["data"]["from_agent"]["id"].as_str() == Some(from),
            "saved handoff sender differs",
        )?;
    }
    Ok(())
}
