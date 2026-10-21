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
        active: &mut [usize; 3 * CALLERS],
        sockets: &mut [usize; CALLERS],
    ) -> Result<(), String> {
        if self.decoded {
            return Err("duplicate decoded reply for one execution".into());
        }
        let write = self.queue < CALLERS;
        if (write && active[self.queue] == 0)
            || self.socket.is_some_and(|caller| sockets[caller] == 0)
        {
            return Err("decoded reply has no held caller/socket credit".into());
        }
        self.decoded = true;
        if write {
            active[self.queue] -= 1;
            self.active_held = false;
        }
        if let Some(caller) = self.socket {
            sockets[caller] -= 1;
            self.socket_held = false;
        }
        Ok(())
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
#[path = "credits/tests.rs"]
mod tests;
