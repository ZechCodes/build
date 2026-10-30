//! Test-only VACUUM progress barrier; never installed in production builds.
use rusqlite::{ffi, Connection};
use std::{
    cell::RefCell,
    marker::PhantomData,
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
    time::Duration,
};

struct State {
    destination: PathBuf,
    reached: Sender<()>,
    release: Receiver<()>,
    signaled: bool,
}

thread_local! {
    static NEXT: RefCell<Option<State>> = const { RefCell::new(None) };
}

pub(in crate::store) fn arm(destination: PathBuf, reached: Sender<()>, release: Receiver<()>) {
    NEXT.with(|next| {
        *next.borrow_mut() = Some(State { destination, reached, release, signaled: false });
    });
}

pub(super) struct Guard<'a> {
    db: *mut ffi::sqlite3,
    _state: Box<State>,
    _connection: PhantomData<&'a Connection>,
}

pub(super) fn install(conn: &Connection) -> Option<Guard<'_>> {
    let mut state = Box::new(NEXT.with(|next| next.borrow_mut().take())?);
    let state_ptr = (&mut *state) as *mut State;
    // SAFETY: the connection outlives Guard, the boxed state stays at a stable
    // address, and Guard unregisters the callback before freeing the state.
    let db = unsafe { conn.handle() };
    unsafe {
        ffi::sqlite3_progress_handler(db, 100, Some(progress), state_ptr.cast());
    }
    Some(Guard { db, _state: state, _connection: PhantomData })
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        // SAFETY: Guard borrows the live connection and unregisters before its
        // boxed callback state is dropped.
        unsafe { ffi::sqlite3_progress_handler(self.db, 0, None, std::ptr::null_mut()) }
    }
}

unsafe extern "C" fn progress(context: *mut std::ffi::c_void) -> std::ffi::c_int {
    // SQLite invokes this synchronously on its connection's owning thread;
    // there is no SQLite re-entry here and panics never cross the FFI boundary.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let state = unsafe { &mut *context.cast::<State>() };
        if !state.signaled && std::fs::metadata(&state.destination).is_ok_and(|metadata| metadata.len() > 0) {
            state.signaled = true;
            let _ = state.reached.send(());
            let _ = state.release.recv_timeout(Duration::from_secs(10));
        }
    }));
    if result.is_ok() { 0 } else { 1 }
}
