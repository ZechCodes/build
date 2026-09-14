//! One filesystem watcher per worktree: the change *producer* the push
//! subscriptions read from (Bridge Wire Protocol Spec, part 1 step 1.2).
//!
//! The old `diff::watch` recomputed a whole `WorktreeDiff` per burst. This one
//! never touches git. It classifies raw `notify` paths and notes kinds on the
//! [`ChangeBus`], which is a leaf-mutex insert:
//!
//! - a path under `.git/` notes [`Kind::Git`] — `index`, `HEAD`, `refs/`,
//!   `MERGE_HEAD`, `logs/HEAD` count; `*.lock` and `objects/` churn is dropped;
//! - any other path notes [`Kind::Files`] with the path relative to the
//!   worktree root, *and* [`Kind::Git`], because a working-tree edit moves
//!   status too;
//! - a path the repository ignores (root `.gitignore`, `.git/info/exclude`,
//!   the user's global excludes) notes [`Kind::Git`] only. A `node_modules`
//!   write is not a change the human reads, but it can move status.
//!
//! Raw events are debounced for [`DEBOUNCE`] before anything is noted, so a
//! build tool rewriting a tree costs one `note_files` rather than thousands.
//! Everything after the debounce is set arithmetic; no git process, no libgit2
//! call, and no lock but the bus's own is taken on the watcher thread.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::Watcher;

use crate::changes::{ChangeBus, Kind};

/// How long a burst of raw filesystem events collapses before it is noted.
/// Short enough that the focused workspace's git pane still updates "within a
/// second of a write", long enough that a compiler's output tree is one note.
pub const DEBOUNCE: Duration = Duration::from_millis(100);

/// Why a worktree could not be watched. The caller answers `watch: "polled"`
/// for this worktree and lets the TTL refresh keep serving it.
#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    /// `notify` refused the path: inotify watch limit, a filesystem it cannot
    /// follow, or a root that does not exist.
    #[error("cannot watch {path}: {message}")]
    Notify {
        /// The worktree root that was asked for.
        path: String,
        /// The underlying `notify` (or path resolution) failure.
        message: String,
    },
}

/// A live watcher on one worktree. Holds the `notify` watcher and the
/// classifying thread; dropping it stops watching — the dropped watcher closes
/// the raw channel, and the thread ends on the next receive.
pub struct WorktreeWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl WorktreeWatcher {
    /// Begin watching `worktree_root` recursively, noting every classified
    /// burst against `entity_id` on `bus`.
    ///
    /// Runs on a blocking task: `notify`'s backend and the classifying thread
    /// are both synchronous.
    pub fn start(
        worktree_root: &Path,
        entity_id: &str,
        bus: Arc<ChangeBus>,
    ) -> Result<WorktreeWatcher, WatchError> {
        start_with_sink(worktree_root, entity_id, bus, DEBOUNCE)
    }
}

/// What the watcher does with a change. Named so the classification is a pure
/// function over a path and the tests can pin every rule without a bus.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    /// Metadata movement only: note `git`, name no file.
    Git,
    /// A working-tree path, relative to the root, plus `git`.
    File(String),
    /// Noise. Nothing is noted.
    Drop,
}

/// Everything one debounced burst amounts to.
#[derive(Debug, Default, PartialEq, Eq)]
struct Burst {
    /// Any classified event at all moves git.
    git: bool,
    /// Deduped, sorted, root-relative working-tree paths.
    files: BTreeSet<String>,
}

/// Where the watcher notes what it saw. [`ChangeBus`] is the only production
/// implementation; the tests record instead, which is what keeps this module
/// free of the bus's flush machinery.
trait ChangeSink: Send + Sync + 'static {
    /// These working-tree paths moved (relative to the worktree root).
    fn note_files(&self, entity_id: &str, paths: &[String]);
    /// Git state for this entity moved.
    fn note_git(&self, entity_id: &str);
}

impl ChangeSink for ChangeBus {
    fn note_files(&self, entity_id: &str, paths: &[String]) {
        ChangeBus::note_files(self, entity_id, paths);
    }

    fn note_git(&self, entity_id: &str) {
        self.note_kind(entity_id, Kind::Git);
    }
}

/// [`WorktreeWatcher::start`] over any sink and debounce — the seam the tests
/// drive.
fn start_with_sink<S: ChangeSink>(
    worktree_root: &Path,
    entity_id: &str,
    sink: Arc<S>,
    debounce: Duration,
) -> Result<WorktreeWatcher, WatchError> {
    let fail = |message: String| WatchError::Notify {
        path: worktree_root.display().to_string(),
        message,
    };
    // Canonical, because notify reports canonical paths and the classifier
    // strips the root off them.
    let root = std::fs::canonicalize(worktree_root).map_err(|err| fail(err.to_string()))?;

    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<Vec<PathBuf>>();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res {
            let _ = raw_tx.send(event.paths);
        }
    })
    .map_err(|err| fail(err.to_string()))?;
    watcher
        .watch(&root, notify::RecursiveMode::Recursive)
        .map_err(|err| fail(err.to_string()))?;

    let classifier = Classifier::for_root(&root);
    let entity_id = entity_id.to_string();
    std::thread::spawn(move || classify_loop(raw_rx, &classifier, &entity_id, &*sink, debounce));

    Ok(WorktreeWatcher { _watcher: watcher })
}

/// Block for the first event of a burst, drain until the filesystem has been
/// quiet for `debounce`, then note the burst once. Ends when the watcher is
/// dropped and the channel closes.
fn classify_loop<S: ChangeSink>(
    raw_rx: Receiver<Vec<PathBuf>>,
    classifier: &Classifier,
    entity_id: &str,
    sink: &S,
    debounce: Duration,
) {
    while let Ok(first) = raw_rx.recv() {
        let mut burst = Burst::default();
        classifier.absorb(&mut burst, &first);
        while let Ok(more) = raw_rx.recv_timeout(debounce) {
            classifier.absorb(&mut burst, &more);
        }
        note(&burst, entity_id, sink);
    }
    // The outer `recv` only fails once the channel is closed: the watcher was
    // dropped, so this thread is done.
}

/// Put one debounced burst on the sink: the named paths first, then git, so a
/// single flush carries both kinds.
fn note<S: ChangeSink>(burst: &Burst, entity_id: &str, sink: &S) {
    if !burst.files.is_empty() {
        let paths: Vec<String> = burst.files.iter().cloned().collect();
        sink.note_files(entity_id, &paths);
    }
    if burst.git {
        sink.note_git(entity_id);
    }
}

/// The root and its ignore rules — everything needed to turn a raw path into a
/// [`Class`].
struct Classifier {
    root: PathBuf,
    /// The root's `.gitignore` and `.git/info/exclude`.
    ignores: Gitignore,
    /// The user's global excludes (`core.excludesFile`).
    global: Gitignore,
}

impl Classifier {
    /// Build the matcher once, at start. A `.gitignore` edited later is itself
    /// a working-tree change; re-reading it per event would put file I/O on the
    /// watcher thread, which is the thing this module refuses to do.
    fn for_root(root: &Path) -> Classifier {
        let mut builder = GitignoreBuilder::new(root);
        // Both `add` calls return the error rather than failing: a repository
        // with no `.gitignore` is the common case, and an unreadable one must
        // not cost the worktree its watcher.
        builder.add(root.join(".gitignore"));
        builder.add(root.join(".git/info/exclude"));
        let ignores = builder.build().unwrap_or_else(|_| Gitignore::empty());
        let (global, _) = Gitignore::global();
        Classifier {
            root: root.to_path_buf(),
            ignores,
            global,
        }
    }

    /// Fold every path of one raw event into the burst.
    fn absorb(&self, burst: &mut Burst, paths: &[PathBuf]) {
        for path in paths {
            match self.classify(path) {
                Class::Drop => {}
                Class::Git => burst.git = true,
                Class::File(relative) => {
                    burst.git = true;
                    burst.files.insert(relative);
                }
            }
        }
    }

    /// What one raw path means.
    fn classify(&self, path: &Path) -> Class {
        let Ok(relative) = path.strip_prefix(&self.root) else {
            return Class::Drop; // outside the worktree; not ours to report
        };
        if let Ok(inside_git) = relative.strip_prefix(".git") {
            return classify_git_metadata(inside_git);
        }
        let text = slash_path(relative);
        if text.is_empty() {
            return Class::Drop; // the root itself
        }
        if self.is_ignored(relative) {
            return Class::Git;
        }
        Class::File(text)
    }

    /// Ignored by the root's rules or by the user's global excludes. Parents
    /// count: `node_modules/` ignores everything beneath it.
    fn is_ignored(&self, relative: &Path) -> bool {
        self.ignores
            .matched_path_or_any_parents(relative, false)
            .is_ignore()
            || self
                .global
                .matched_path_or_any_parents(relative, false)
                .is_ignore()
    }
}

/// A path under `.git/`, relative to `.git/` itself. `index`, `HEAD`, `refs/`,
/// `MERGE_HEAD` and `logs/HEAD` are real movement; loose-object writes and any
/// lock file are the churn every one of those writes drags behind it.
fn classify_git_metadata(inside_git: &Path) -> Class {
    let text = slash_path(inside_git);
    if text.is_empty() {
        return Class::Drop; // the `.git` directory's own mtime
    }
    if text.ends_with(".lock") || text.starts_with("objects/") || text == "objects" {
        return Class::Drop;
    }
    Class::Git
}

/// A relative path as the wire spells it: `/`-separated, never `\`.
fn slash_path(relative: &Path) -> String {
    relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Instant;

    /// A sink that remembers, so a test can assert on what the watcher decided
    /// without standing up the bus's flush loop.
    #[derive(Default)]
    struct Recorder {
        files: Mutex<Vec<Vec<String>>>,
        git: Mutex<usize>,
    }

    impl Recorder {
        fn noted_paths(&self) -> Vec<String> {
            self.files
                .lock()
                .unwrap()
                .iter()
                .flatten()
                .cloned()
                .collect()
        }

        fn git_notes(&self) -> usize {
            *self.git.lock().unwrap()
        }
    }

    impl ChangeSink for Recorder {
        fn note_files(&self, _entity_id: &str, paths: &[String]) {
            self.files.lock().unwrap().push(paths.to_vec());
        }

        fn note_git(&self, _entity_id: &str) {
            *self.git.lock().unwrap() += 1;
        }
    }

    /// A tempdir repository: a real `.git`, one committed file, so the ignore
    /// matcher and the `.git/` rules both have something true to work against.
    fn repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        git2::Repository::init(root).expect("git init");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".gitignore"), "node_modules/\n*.log\n").unwrap();
        dir
    }

    fn start_recording(root: &Path) -> (WorktreeWatcher, Arc<Recorder>) {
        let sink = Arc::new(Recorder::default());
        let watcher = start_with_sink(root, "run-7", sink.clone(), Duration::from_millis(60))
            .expect("watcher starts on a real repo");
        // notify's backend registers asynchronously on some platforms; give it
        // a beat so the write under test is not missed.
        std::thread::sleep(Duration::from_millis(150));
        (watcher, sink)
    }

    /// Poll until `ready` or the deadline; returns whether it came true.
    fn settle(sink: &Recorder, ready: impl Fn(&Recorder) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if ready(sink) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn a_working_tree_write_notes_the_path_and_git() {
        let dir = repo();
        let (_watcher, sink) = start_recording(dir.path());

        std::fs::write(dir.path().join("src/lib.rs"), "fn main() {}\n").unwrap();

        assert!(
            settle(&sink, |s| s
                .noted_paths()
                .contains(&"src/lib.rs".to_string())),
            "expected src/lib.rs, got {:?}",
            sink.noted_paths()
        );
        assert!(sink.git_notes() >= 1, "a working-tree edit moves git too");
    }

    #[test]
    fn object_churn_notes_nothing() {
        let dir = repo();
        let (_watcher, sink) = start_recording(dir.path());

        let objects = dir.path().join(".git/objects/ab");
        std::fs::create_dir_all(&objects).unwrap();
        std::fs::write(objects.join("cdef0123456789"), b"loose object").unwrap();
        std::fs::write(dir.path().join(".git/index.lock"), b"").unwrap();

        assert!(
            !settle(&sink, |s| s.git_notes() > 0 || !s.noted_paths().is_empty()),
            "objects/ and *.lock churn is dropped, got {:?}/{}",
            sink.noted_paths(),
            sink.git_notes()
        );
    }

    #[test]
    fn git_metadata_notes_git_with_no_path() {
        let dir = repo();
        let (_watcher, sink) = start_recording(dir.path());

        std::fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();

        assert!(settle(&sink, |s| s.git_notes() > 0), "HEAD counts");
        assert!(
            sink.noted_paths().is_empty(),
            "no working-tree path for a .git write, got {:?}",
            sink.noted_paths()
        );
    }

    #[test]
    fn an_ignored_path_notes_git_only() {
        let dir = repo();
        let (_watcher, sink) = start_recording(dir.path());

        std::fs::create_dir_all(dir.path().join("node_modules/left-pad")).unwrap();
        std::fs::write(
            dir.path().join("node_modules/left-pad/index.js"),
            "module.exports = 1\n",
        )
        .unwrap();

        assert!(
            settle(&sink, |s| s.git_notes() > 0),
            "an ignored write can still move status"
        );
        assert!(
            !sink
                .noted_paths()
                .iter()
                .any(|path| path.starts_with("node_modules/")),
            "ignored paths are not files the human reads, got {:?}",
            sink.noted_paths()
        );
    }

    #[test]
    fn start_on_an_unwatchable_path_is_an_error() {
        let dir = repo();
        let missing = dir.path().join("no-such-worktree");

        let sink = Arc::new(Recorder::default());
        let Err(err) = start_with_sink(&missing, "run-7", sink, DEBOUNCE) else {
            panic!("a path that cannot be watched must not yield a watcher");
        };

        let WatchError::Notify { path, .. } = &err;
        assert!(path.contains("no-such-worktree"), "{err}");
    }

    #[test]
    fn dropping_the_watcher_stops_noting() {
        let dir = repo();
        let (watcher, sink) = start_recording(dir.path());
        drop(watcher);
        std::thread::sleep(Duration::from_millis(100));

        std::fs::write(dir.path().join("src/after.rs"), "// late\n").unwrap();

        assert!(
            !settle(&sink, |s| !s.noted_paths().is_empty()),
            "a dropped watcher notes nothing, got {:?}",
            sink.noted_paths()
        );
    }

    #[test]
    fn classification_rules() {
        let dir = repo();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let classifier = Classifier::for_root(&root);

        let cases = [
            ("src/lib.rs", Class::File("src/lib.rs".into())),
            (".git/index", Class::Git),
            (".git/refs/heads/main", Class::Git),
            (".git/logs/HEAD", Class::Git),
            (".git/MERGE_HEAD", Class::Git),
            (".git/index.lock", Class::Drop),
            (".git/refs/heads/main.lock", Class::Drop),
            (".git/objects/ab/cdef", Class::Drop),
            ("node_modules/x/index.js", Class::Git),
            ("build.log", Class::Git),
        ];
        for (relative, want) in cases {
            assert_eq!(
                classifier.classify(&root.join(relative)),
                want,
                "{relative}"
            );
        }
        assert_eq!(classifier.classify(Path::new("/elsewhere/x")), Class::Drop);
    }
}
