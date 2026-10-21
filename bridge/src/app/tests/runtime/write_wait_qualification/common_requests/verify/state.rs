use super::*;
pub(super) fn verify(
    request: &CommonRequest,
    result: &Value,
    actual: SavedEffect,
) -> Result<OracleWitness, String> {
    match actual {
        SavedEffect::Comment(_) => comment(request, result, actual),
        SavedEffect::UserSession { .. } => presence(request, result, actual),
        SavedEffect::Seen { .. } => seen(request, result, actual),
        _ => Err("native saved witness type does not match actual request effect".into()),
    }
}
fn comment(
    request: &CommonRequest,
    result: &Value,
    actual: SavedEffect,
) -> Result<OracleWitness, String> {
    let mut fields = Vec::new();
    match (&request.effect, actual) {
        (ExpectedEffect::Comment { task, body, actor }, SavedEffect::Comment(saved)) => {
            ensure(
                saved.task_id == *task
                    && saved.body == *body
                    && actor_matches(&saved.author, actor),
                "actual saved comment task/body/actor differs",
            )?;
            ensure(
                result["comment"]["id"].as_str() == Some(saved.id.as_str())
                    && result["comment"]["body"].as_str() == Some(body),
                "returned exact comment ID/body differs from saved comment",
            )?;
            fields.push(assertion("saved_comment_id", WitnessScalar::Text(saved.id)));
            fields.push(assertion(
                "saved_comment_body",
                WitnessScalar::Text(saved.body),
            ));
        }
        _ => return Err("native saved witness type does not match actual request effect".into()),
    }
    Ok(state_witness(request, fields))
}
fn presence(
    request: &CommonRequest,
    result: &Value,
    actual: SavedEffect,
) -> Result<OracleWitness, String> {
    let mut fields = Vec::new();
    match (&request.effect, actual) {
        (
            ExpectedEffect::UserPresent,
            SavedEffect::UserSession {
                persisted,
                invocation_start_ms,
                decoded_final_ms,
            },
        ) => {
            let replied_session: crate::session_summary::UserSession =
                serde_json::from_value(result["user_session"].clone())
                    .map_err(|error| format!("actual presence reply session invalid: {error}"))?;
            let replied = replied_session
                .last_activity_ms
                .ok_or("actual presence reply lacks activity time")?;
            ensure(
                invocation_start_ms <= replied
                    && replied <= decoded_final_ms
                    && persisted == replied_session,
                "actual presence session differs in an original field or falls outside invocation",
            )?;
            fields.push(assertion(
                "replied_activity_ms",
                WitnessScalar::I64(replied),
            ));
            fields.push(assertion(
                "persisted_activity_ms",
                WitnessScalar::I64(persisted.last_activity_ms.unwrap()),
            ));
        }
        _ => return Err("native saved witness type does not match actual request effect".into()),
    }
    Ok(state_witness(request, fields))
}
fn seen(
    request: &CommonRequest,
    _result: &Value,
    actual: SavedEffect,
) -> Result<OracleWitness, String> {
    let mut fields = Vec::new();
    match (&request.effect, actual) {
        (
            ExpectedEffect::Seen {
                owner,
                agent,
                thread,
            },
            SavedEffect::Seen {
                owner: actual_owner,
                agent: actual_agent,
                thread: actual_thread,
                persisted_through,
                published_through,
                actual_read_target_through,
            },
        ) => {
            ensure(owner == &actual_owner && agent == &actual_agent && thread == &actual_thread && persisted_through >= actual_read_target_through && published_through == persisted_through, "actual saved/published seen cursor does not cover actual targeted canonical sequence")?;
            fields.push(assertion(
                "actual_read_target_through",
                WitnessScalar::U64(actual_read_target_through),
            ));
            fields.push(assertion(
                "saved_and_published_through",
                WitnessScalar::U64(persisted_through),
            ));
        }
        _ => return Err("native saved witness type does not match actual request effect".into()),
    }
    Ok(state_witness(request, fields))
}
