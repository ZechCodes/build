use super::*;
impl Builder<'_> {
    pub(super) fn file(&mut self, context: &SlotContext<'_>) -> Result<Parts, String> {
        let slot = context.slot;
        let id = slot.original_id.clone();
        let owner = slot.owner;
        let target = context.target;
        let predecessor = self.last_file[owner].replace(id.clone());
        let bytes = body("finite filesystem edit", &id)?.into_bytes();
        let revision = sha256(&bytes);
        let mut params = file_scope(target);
        params["path"] = json!("qualified-editor.txt");
        params["expected_revision"] = json!(self.file_revision[owner]);
        params["content_b64"] = json!(crate::encoding::b64encode(&bytes));
        // This is the planned successful-chain revision. A predecessor
        // failure blocks dependents; it NEVER rereads/retries/repairs.
        self.file_revision[owner] = revision.clone();
        let (request, effect) = (
            ActualRequest::Rpc {
                method: "fs.write",
                params,
            },
            ExpectedEffect::File {
                owner,
                path: target.path.join("qualified-editor.txt"),
                bytes,
                revision,
            },
        );
        Ok(Parts {
            request,
            effect,
            durable_operation_id: None,
            predecessor,
        })
    }
    pub(super) fn git(&mut self, context: &SlotContext<'_>) -> Result<Parts, String> {
        let targets = self.targets;
        let slot = context.slot;
        let id = slot.original_id.clone();
        let owner = slot.owner;
        let target = context.target;
        let predecessor = self.last_git[owner].replace(id.clone());
        let full_ref = targets.git[owner].full_refs[self.git_sequence[owner] % 2].clone();
        self.git_sequence[owner] += 1;
        let mut params = file_scope(target);
        params["full_ref"] = json!(full_ref);
        let (request, effect) = (
            ActualRequest::Rpc {
                method: "git.checkout_ref",
                params,
            },
            ExpectedEffect::GitHead {
                owner,
                directory: target.path.clone(),
                full_ref,
                commit: targets.git[owner].initial_commit.clone(),
            },
        );
        Ok(Parts {
            request,
            effect,
            durable_operation_id: None,
            predecessor,
        })
    }
}
