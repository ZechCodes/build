//! Service-time injection scoped to the calling test thread.
use std::cell::RefCell;
thread_local! {
    static SERVICE: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
}
pub(in crate::app) fn install(service: impl FnMut() + 'static) {
    SERVICE.with(|slot| *slot.borrow_mut() = Some(Box::new(service)));
}
pub(in crate::app) fn service() {
    SERVICE.with(|slot| {
        if let Some(service) = slot.borrow_mut().as_mut() {
            service();
        }
    });
}
pub(in crate::app) fn clear() {
    SERVICE.with(|slot| *slot.borrow_mut() = None);
}
