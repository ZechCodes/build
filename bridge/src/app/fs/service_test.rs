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
thread_local! {
    static BEFORE_PUBLISH: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
    static AFTER_PUBLISH_CLAIM: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
    static AFTER_REPLACE: RefCell<Option<Box<dyn FnMut()>>> = RefCell::new(None);
}

pub(in crate::app) fn install_before_publish(service: impl FnMut() + 'static) {
    BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = Some(Box::new(service)));
}
pub(in crate::app) fn before_publish() {
    BEFORE_PUBLISH.with(|slot| {
        if let Some(service) = slot.borrow_mut().as_mut() {
            service();
        }
    });
}
pub(in crate::app) fn clear_before_publish() {
    BEFORE_PUBLISH.with(|slot| *slot.borrow_mut() = None);
}

pub(in crate::app) fn install_after_publish_claim(service: impl FnMut() + 'static) {
    AFTER_PUBLISH_CLAIM.with(|slot| *slot.borrow_mut() = Some(Box::new(service)));
}
pub(in crate::app) fn after_publish_claim() {
    AFTER_PUBLISH_CLAIM.with(|slot| {
        if let Some(service) = slot.borrow_mut().as_mut() {
            service();
        }
    });
}
pub(in crate::app) fn clear_after_publish_claim() {
    AFTER_PUBLISH_CLAIM.with(|slot| *slot.borrow_mut() = None);
}

pub(in crate::app) fn install_after_replace(service: impl FnMut() + 'static) {
    AFTER_REPLACE.with(|slot| *slot.borrow_mut() = Some(Box::new(service)));
}
pub(in crate::app) fn after_replace() {
    AFTER_REPLACE.with(|slot| {
        if let Some(service) = slot.borrow_mut().as_mut() {
            service();
        }
    });
}
pub(in crate::app) fn clear_after_replace() {
    AFTER_REPLACE.with(|slot| *slot.borrow_mut() = None);
}
