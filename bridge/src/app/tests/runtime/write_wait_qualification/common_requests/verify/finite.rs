use super::*;
pub(super) fn verify(
    request: &CommonRequest,
    result: &Value,
    actual: SavedEffect,
) -> Result<OracleWitness, String> {
    let mut fields = Vec::new();
    match (&request.effect, actual) {
        (
            ExpectedEffect::File {
                path,
                bytes,
                revision,
            },
            SavedEffect::File {
                actual_path,
                actual_bytes,
            },
        ) => {
            ensure(
                path == &actual_path && bytes == &actual_bytes,
                "actual filesystem bytes/path differ",
            )?;
            ensure(
                result["revision"].as_str() == Some(revision)
                    && result["size"].as_u64() == Some(bytes.len() as u64)
                    && result["content_b64"].as_str()
                        == Some(crate::encoding::b64encode(bytes).as_str()),
                "actual fs.write reply revision/size/content differs",
            )?;
            ensure(
                &sha256(&actual_bytes) == revision,
                "actual written revision differs",
            )?;
            return Ok(OracleWitness::File {
                oracle: "actual-file-bytes+returned-revision".into(),
                path_suffix: "qualified-editor.txt".into(),
                bytes: bytes.len() as u64,
                sha256: revision.clone(),
            });
        }
        (
            ExpectedEffect::GitHead {
                directory,
                full_ref,
                commit,
            },
            SavedEffect::GitHead {
                actual_directory,
                actual_symbolic_head,
                actual_commit,
            },
        ) => {
            ensure(
                directory == &actual_directory
                    && full_ref == &actual_symbolic_head
                    && commit == &actual_commit,
                "actual libgit symbolic HEAD/commit differs",
            )?;
            fields.push(assertion(
                "actual_symbolic_head",
                WitnessScalar::Text(actual_symbolic_head),
            ));
            fields.push(assertion(
                "actual_commit",
                WitnessScalar::Text(actual_commit),
            ));
        }
        _ => return Err("native saved witness type does not match actual request effect".into()),
    }
    Ok(state_witness(request, fields))
}
