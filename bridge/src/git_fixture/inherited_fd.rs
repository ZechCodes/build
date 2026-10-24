//! Hold the pre-exec fork window open without touching Rust state in the child.
use std::fs::File;
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::{Duration, Instant};

pub struct InheritedFdChild(libc::pid_t);

pub fn assert_cloexec(file: &File) {
    // SAFETY: F_GETFD only reads flags from this live file descriptor.
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFD) };
    assert!(flags >= 0 && flags & libc::FD_CLOEXEC != 0);
}

impl InheritedFdChild {
    pub fn hold() -> Self {
        let mut ready = [0; 2];
        // SAFETY: pipe2 writes two descriptors into the supplied array.
        assert_eq!(
            unsafe { libc::pipe2(ready.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // SAFETY: the child uses only async-signal-safe syscalls until killed.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0);
        if pid == 0 {
            unsafe {
                libc::close(ready[0]);
                libc::write(ready[1], b"x".as_ptr().cast(), 1);
                libc::close(ready[1]);
                loop {
                    libc::pause();
                }
            }
        }
        let child = Self(pid);
        // SAFETY: the parent owns both descriptors and closes each exactly once.
        unsafe { libc::close(ready[1]) };
        let _reader = unsafe { File::from_raw_fd(ready[0]) };
        let mut poll = libc::pollfd {
            fd: ready[0],
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one valid pollfd and a five-second deadline.
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 5000) }, 1);
        assert_ne!(poll.revents & libc::POLLIN, 0);
        child
    }
}

impl Drop for InheritedFdChild {
    fn drop(&mut self) {
        // SAFETY: this unreaped PID is this guard's own child.
        unsafe { libc::kill(self.0, libc::SIGKILL) };
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            // SAFETY: waitpid only observes our own child.
            let result = unsafe { libc::waitpid(self.0, std::ptr::null_mut(), libc::WNOHANG) };
            if result == self.0
                || (result < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD))
            {
                return;
            }
            assert!(Instant::now() < deadline, "inherited-FD child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}
