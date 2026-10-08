use super::*;
use crate::reviews::merge::tests::job;
use crate::reviews::sync::reconcile::tests::Fixture;
use std::os::fd::FromRawFd;

struct InheritedLeaseChild(libc::pid_t);

impl InheritedLeaseChild {
    fn hold() -> Self {
        let mut ready = [0; 2];
        // SAFETY: pipe2 receives two writable integer slots.
        assert_eq!(
            unsafe { libc::pipe2(ready.as_mut_ptr(), libc::O_CLOEXEC) },
            0
        );
        // SAFETY: the child uses only async-signal-safe libc calls until killed.
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
        // SAFETY: the parent owns both pipe FDs and closes each once.
        unsafe { libc::close(ready[1]) };
        let _reader = unsafe { std::fs::File::from_raw_fd(ready[0]) };
        let mut poll = libc::pollfd {
            fd: ready[0],
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: poll receives one valid pollfd and a bounded deadline.
        assert_eq!(unsafe { libc::poll(&mut poll, 1, 2000) }, 1);
        assert_ne!(poll.revents & libc::POLLIN, 0);
        child
    }
}

impl Drop for InheritedLeaseChild {
    fn drop(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(2);
        // SAFETY: this unreaped PID is this test's own forked child.
        unsafe { libc::kill(self.0, libc::SIGKILL) };
        while Instant::now() < deadline {
            // SAFETY: waiting on our own child does not access any Rust data.
            let result = unsafe { libc::waitpid(self.0, std::ptr::null_mut(), libc::WNOHANG) };
            if result == self.0
                || (result < 0
                    && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD))
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            std::thread::panicking(),
            "inherited-lease child was not reaped before its deadline"
        );
    }
}

#[test]
fn a_completed_merge_lease_releases_ownership_while_an_unrelated_child_retains_its_fd() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = Fixture::new();
    f.commit("included.txt");
    f.push();
    f.sync();
    let original = lease(&f.review()).unwrap();
    assert!(lease(&f.review()).err().unwrap().starts_with("busy:"));
    let _inherited = InheritedLeaseChild::hold();
    drop(original);
    let merged = merge(&f.store, &job(&f), || {}).unwrap();
    assert_eq!(
        merged.pull_request.unwrap().status,
        crate::reviews::model::PullRequestStatus::Merged
    );
}

#[test]
fn an_error_return_releases_the_merge_lease_despite_an_inherited_fd() {
    crate::git_fixture::environment::isolated_git_test!();
    let f = Fixture::new();
    let mut inherited = None;
    let failed: Result<(), &str> = (|| {
        let _lease = lease(&f.review()).unwrap();
        inherited = Some(InheritedLeaseChild::hold());
        Err("intent persistence refused")
    })();
    assert_eq!(failed, Err("intent persistence refused"));
    let replacement = lease(&f.review()).unwrap();
    assert!(lease(&f.review()).err().unwrap().starts_with("busy:"));
    drop(replacement);
    assert!(lease(&f.review()).is_ok());
    drop(inherited);
}
