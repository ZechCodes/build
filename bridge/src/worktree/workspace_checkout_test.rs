//! Fault injection at the checkout/unwind seam; no production behaviour.
use crate::isolation::WorktreeError;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Boundary {
    Materialize,
    Teardown,
    BeforeRemove,
    AfterRemove(bool),
    BeforeRefRemoval,
    AfterRefLookup(Option<git2::ErrorCode>),
}
type Observer = Box<dyn FnMut(Boundary, &Path, &str) -> Result<(), WorktreeError>>;
thread_local! {
    static OBSERVER: std::cell::RefCell<Option<Observer>> = const { std::cell::RefCell::new(None) };
}
pub(crate) struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        OBSERVER.with_borrow_mut(|saved| *saved = None);
    }
}
pub(crate) fn observe(
    observer: impl FnMut(Boundary, &Path, &str) -> Result<(), WorktreeError> + 'static,
) -> Guard {
    OBSERVER.with_borrow_mut(|saved| *saved = Some(Box::new(observer)));
    Guard
}
pub(crate) fn at(boundary: Boundary, path: &Path, branch: &str) -> Result<(), WorktreeError> {
    OBSERVER.with_borrow_mut(|saved| match saved {
        Some(observer) => observer(boundary, path, branch),
        None => Ok(()),
    })
}
