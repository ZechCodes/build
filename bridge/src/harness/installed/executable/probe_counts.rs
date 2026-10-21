//! Current-thread attempts at executable identity's actual filesystem calls.

use std::cell::Cell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub metadata_attempts: usize,
    pub canonicalize_attempts: usize,
}

impl Counts {
    pub(crate) fn total(self) -> usize {
        self.metadata_attempts + self.canonicalize_attempts
    }
}

thread_local! {
    static COUNTS: Cell<Counts> = const { Cell::new(Counts {
        metadata_attempts: 0,
        canonicalize_attempts: 0,
    }) };
}

struct Restore(Counts);

impl Drop for Restore {
    fn drop(&mut self) {
        COUNTS.set(self.0);
    }
}

/// Nested measurements isolate their attempts and restore the enclosing count,
/// including when the measured closure unwinds. Work on other threads is not
/// included; callers measure admission, work and finalization where they run.
pub(crate) fn measure<T>(work: impl FnOnce() -> T) -> (T, Counts) {
    let _restore = Restore(COUNTS.replace(Counts::default()));
    let result = work();
    (result, COUNTS.get())
}

pub(super) fn metadata_attempt() {
    COUNTS.update(|mut count| {
        count.metadata_attempts += 1;
        count
    });
}

pub(super) fn canonicalize_attempt() {
    COUNTS.update(|mut count| {
        count.canonicalize_attempts += 1;
        count
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::installed::executable::identify_on_path;
    use std::ffi::OsStr;

    #[test]
    fn executable_probe_counts_include_missing_paths_and_successful_resolution() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir(&bin).unwrap();
        let binary = bin.join("fixture-cli");
        crate::isolation::test_fixture::write_executable(&binary, "#!/bin/sh\nexit 0\n");

        let (missing, counts) =
            measure(|| identify_on_path("missing", OsStr::new("bin:absent"), home.path()));
        assert!(missing.is_none());
        assert_eq!(
            counts.metadata_attempts, 2,
            "every PATH miss attempts metadata"
        );
        assert_eq!(counts.canonicalize_attempts, 0);
        assert_eq!(counts.total(), 2);

        let (found, counts) =
            measure(|| identify_on_path("fixture-cli", OsStr::new("bin"), home.path()));
        assert_eq!(found.unwrap().path(), binary);
        assert_eq!(counts.metadata_attempts, 1);
        assert_eq!(counts.canonicalize_attempts, 1);
        assert_eq!(counts.total(), 2);
    }

    #[test]
    fn executable_probe_measurements_restore_nested_counts_after_unwind() {
        let home = tempfile::tempdir().unwrap();
        let missing = || identify_on_path("missing", OsStr::new("absent"), home.path());
        let ((), outer) = measure(|| {
            assert!(missing().is_none());
            let (value, inner) = measure(missing);
            assert!(value.is_none());
            assert_eq!(inner.metadata_attempts, 1);
            let unwound = std::panic::catch_unwind(|| {
                measure(|| {
                    assert!(missing().is_none());
                    panic!("fixture probe unwind");
                });
            });
            assert!(unwound.is_err());
            assert!(missing().is_none());
        });
        assert_eq!(outer.metadata_attempts, 2, "nested attempts stay isolated");
        assert_eq!(outer.canonicalize_attempts, 0);
    }
}
