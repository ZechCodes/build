//! Caller concurrency is separate from execution/proof task ownership.
use super::*;

pub(super) struct CallerCredits {
    queue: usize,
    socket: Option<usize>,
    active_held: bool,
    socket_held: bool,
    decoded: bool,
}

impl CallerCredits {
    pub(super) fn acquired(queue: usize, socket: Option<usize>) -> Self {
        Self {
            queue,
            socket,
            active_held: true,
            socket_held: socket.is_some(),
            decoded: false,
        }
    }

    pub(super) fn decoded(
        &mut self,
        _active: &mut [usize; 3 * CALLERS],
        _sockets: &mut [usize; CALLERS],
    ) -> Result<(), String> {
        // TESTFIRST: old coordinator behavior retains every credit until join.
        Err("decoded reply credit release is not implemented".into())
    }

    pub(super) fn finished(
        self,
        active: &mut [usize; 3 * CALLERS],
        sockets: &mut [usize; CALLERS],
    ) {
        if self.active_held {
            active[self.queue] -= 1;
        }
        if self.socket_held {
            sockets[self.socket.expect("held socket belongs to a caller")] -= 1;
        }
    }
}

#[cfg(test)]
mod tests;
