//! A terminal interaction patches only its accepted attention rows. Persistence
//! carries the exact interaction pair to a worker without delaying the reply.
use super::AppState;
use crate::app::read_view::shared_json::AttentionPatch;
use crate::app::read_view::SnapshotDelta;
use std::sync::Arc;

impl AppState {
    pub(in crate::app) fn touch_terminal_attention(
        &mut self,
        owner: &str,
    ) -> Option<TerminalAttentionSave> {
        #[cfg(test)]
        let _phase = crate::timing::phase_samples::phase("term.create.attention");
        let now = crate::store::now_rfc3339();
        self.board.attention_mut().interact(owner, &now);
        let attention = self.board.attention().attention(owner).unwrap();
        let last = attention.last_interaction_at.clone().unwrap();
        let resume = attention.resume_at.clone();
        let rendered_attention = self.attention_json(owner);
        self.read_view.publish_current(|previous, version| {
            let mut patch = AttentionPatch::new(owner, rendered_attention.clone());
            let mut facts = previous.facts.as_ref().clone();
            facts.patch_terminal_attention(&mut patch);
            let mut delta = SnapshotDelta::new(version).set_facts(Arc::new(facts));
            for key in previous.keys().filter(|key| {
                matches!(
                    key.method,
                    "board.list" | "project.list" | "workspace.list" | "workspace.get"
                )
            }) {
                let row = previous.get_shared(key.method, &key.owner)?;
                if let Some(row) = patch.apply(row) {
                    delta = delta.put_shared(key.method, &key.owner, row);
                }
            }
            if let Some(global) = &previous.global {
                let next =
                    global.patch_shared(&mut |row| patch.apply(row).unwrap_or_else(|| row.clone()));
                delta = delta.set_global(Arc::new(next));
            }
            Some(delta)
        });
        self.changes.note_kind(owner, crate::changes::Kind::State);
        self.changes.note_board_lists(Default::default());
        let store = self.store.clone()?;
        let external_epoch =
            crate::worktree::is_checkout_id(owner).then(|| store.terminal_attention_epoch());
        Some(TerminalAttentionSave {
            store,
            external_epoch,
            durable_owner: self.runs.contains_key(owner) || self.plans.contains_key(owner),
            owner: owner.to_owned(),
            last,
            resume,
        })
    }
}

pub(in crate::app) struct TerminalAttentionSave {
    store: crate::store::Store,
    owner: String,
    last: String,
    resume: Option<String>,
    durable_owner: bool,
    external_epoch: Option<u64>,
}

impl TerminalAttentionSave {
    /// Both thread creation and SQLite access happen after the app guard drops.
    pub(in crate::app) fn spawn(self) {
        if let Err(error) = std::thread::Builder::new()
            .name("terminal-attention".into())
            .spawn(move || {
                if let Err(error) = self.store.save_terminal_interaction(
                    &self.owner,
                    &self.last,
                    self.resume.as_deref(),
                    self.durable_owner,
                    self.external_epoch,
                ) {
                    eprintln!("terminal attention persist failed: {error}");
                }
            })
        {
            eprintln!("terminal attention worker failed: {error}");
        }
    }
}
