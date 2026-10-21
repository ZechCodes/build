use super::*;
use crate::timing::write_wait::{current_context, Obligation, SyncContextGuard};

thread_local! { static INTENT: RefCell<Vec<WriteContext>> = const { RefCell::new(Vec::new()) }; }
pub(super) fn intent_context() -> Option<WriteContext> {
    INTENT
        .with(|intent| intent.borrow().last().cloned())
        .or_else(current_context)
}
pub(in crate::app) struct PublicationStage {
    pub(super) ledger: Arc<PublicationDebts>,
    pub(super) owner: String,
    pub(super) revision: u64,
    pub(super) work: Option<PublicationWork>,
}
impl Drop for PublicationStage {
    fn drop(&mut self) {
        self.ledger.end_stage(&self.owner, self.revision);
        if let Some(work) = self.work.take() {
            if std::thread::panicking() {
                drop(work);
            } else {
                work.complete();
            }
        }
    }
}

pub(in crate::app) struct PublicationWork {
    ledger: Arc<PublicationDebts>,
    receipts: Vec<PublicationReceipt>,
    binding: Option<SyncContextGuard>,
    token: Option<Obligation>,
    intent_bound: bool,
}
impl PublicationWork {
    pub(super) fn bind(
        ledger: Arc<PublicationDebts>,
        receipts: Vec<PublicationReceipt>,
        mut contexts: Vec<WriteContext>,
    ) -> Self {
        let intent = intent_context();
        let (token, binding, intent_bound) = if contexts.is_empty() {
            (None, None, false)
        } else {
            if let Some(current) = current_context() {
                contexts.push(current);
            }
            let token = WriteContext::coalesced(&contexts);
            let context = token.context();
            INTENT.with(|stack| {
                stack
                    .borrow_mut()
                    .push(intent.unwrap_or_else(|| context.clone()))
            });
            let binding = context.bind_sync();
            (Some(token), Some(binding), true)
        };
        Self {
            ledger,
            receipts,
            binding,
            token,
            intent_bound,
        }
    }
    pub(in crate::app) fn complete(mut self) {
        drop(self.binding.take());
        if self.intent_bound {
            INTENT.with(|stack| {
                stack.borrow_mut().pop();
            });
            self.intent_bound = false;
        }
        self.ledger.end_work(&self.receipts);
        self.receipts.clear();
        if let Some(token) = self.token.take() {
            token.complete();
        }
    }
}
impl Drop for PublicationWork {
    fn drop(&mut self) {
        drop(self.binding.take());
        if self.intent_bound {
            INTENT.with(|stack| {
                stack.borrow_mut().pop();
            });
        }
        self.ledger.end_work(&self.receipts);
        // An unwinding scope drops its token as abandoned instead of silently
        // reporting successful publisher completion.
    }
}
