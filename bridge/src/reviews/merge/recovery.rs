use super::*;

/// Startup never replays Git. Only a vector backed by saved successful local
/// steps can reach finalization; uncertain work remains an explicit retry.
pub fn recover(store: &Store) -> Result<usize, String> {
    let intents = store
        .load_running_review_merge_intents()
        .map_err(|error| error.to_string())?;
    let mut recovered = 0;
    for mut intent in intents {
        let review = load(store, &intent.request.task_id)?;
        let _lease = match fence::lease(&review) {
            Ok(lease) => lease,
            Err(error) if error.starts_with("busy:") => continue,
            Err(error) => {
                interrupt(
                    store,
                    &mut intent,
                    &format!("Interrupted: merge receiver unavailable: {error}"),
                )?;
                recovered += 1;
                continue;
            }
        };
        execution::interrupt_rows(
            store,
            &mut intent,
            "Interrupted: check saved Git results before retrying",
        )?;
        let review = load(store, &intent.request.task_id)?;
        let integrated = intent
            .request
            .sources
            .iter()
            .all(|source| successful_merge(&review, &intent, &source.directory_id).is_some());
        if integrated {
            finalize_and_settle(store, &mut intent, &|| {}, &|_| Ok(()))?;
        } else {
            intent.state = ReviewMergeState::Interrupted;
            intent.error =
                Some("Interrupted: Git work is uncertain or incomplete; retry explicitly".into());
            store
                .save_review_merge_intent(&intent, intent.version)
                .map_err(|error| error.to_string())?;
        }
        recovered += 1;
    }
    Ok(recovered)
}
