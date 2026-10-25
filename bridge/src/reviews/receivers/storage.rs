//! Independent object links share disk blocks without depending on source refs.
//!
//! Only immutable pack/loose contents share inodes. Indexes and keep markers
//! belong to the receiver. Optional seeding rolls back on incomplete graphs,
//! cross-device links, or its deadline; normal publication fetch remains the
//! self-contained fallback. This runs during the existing deferred Git job.

use super::ReviewReceiver;
use crate::git_process::run_command_with_deadline;
use std::ffi::OsStr;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const SEED_BUDGET: Duration = Duration::from_secs(5);
const KEEP_MARKER: &[u8] = b"Build shared immutable pack\n";

pub(super) fn seed(receiver: &ReviewReceiver) -> Result<(), String> {
    let receiver = receiver.clone();
    let worker = std::thread::Builder::new()
        .name("review-object-seed".into())
        .spawn(move || {
            crate::priority::lower_this_thread(crate::priority::CHILD_NICE);
            seed_with(&receiver, |source, destination| {
                fs::hard_link(source, destination)
            })
        });
    match worker {
        Ok(worker) => worker
            .join()
            .map_err(|_| "review object seeding panicked")?,
        Err(_) => Ok(()), // An optional optimization must not prevent opening.
    }
}

pub(super) fn seed_with(
    receiver: &ReviewReceiver,
    link: impl Fn(&Path, &Path) -> io::Result<()>,
) -> Result<(), String> {
    let mut seed = Seed {
        files: Vec::new(),
        deadline: Instant::now() + SEED_BUDGET,
        link,
    };
    if seed.import(receiver).is_err() {
        // No ref exists in this unpublished staging repository. Removing only
        // the exact files created here restores the empty-fetch fallback.
        for path in seed.files.iter().rev() {
            fs::remove_file(path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

struct Seed<F> {
    files: Vec<PathBuf>,
    deadline: Instant,
    link: F,
}

impl<F: Fn(&Path, &Path) -> io::Result<()>> Seed<F> {
    fn remaining(&self) -> io::Result<Duration> {
        self.deadline
            .checked_duration_since(Instant::now())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "object seed deadline"))
    }

    fn import(&mut self, receiver: &ReviewReceiver) -> io::Result<()> {
        let source = receiver.source_common_git_dir.join("objects");
        let destination = receiver.path.join("objects");
        if !is_directory(&source) {
            return Ok(());
        }
        for entry in fs::read_dir(&source)? {
            self.remaining()?;
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if is_hex(&name, 2) && is_directory(&entry.path()) {
                self.loose_directory(&entry.path(), &destination.join(name.as_ref()))?;
            }
        }
        self.packs(&source.join("pack"), &destination.join("pack"))?;
        let roots = receiver.path.join("packed-refs");
        self.write_seed_roots(&receiver.path, &roots)?;
        let args: Vec<&OsStr> = [
            "-n",
            "10",
            "git",
            "-c",
            "core.hooksPath=/dev/null",
            "fsck",
            "--connectivity-only",
            "--no-dangling",
            "--no-reflogs",
        ]
        .iter()
        .map(OsStr::new)
        .collect();
        let output = run_command_with_deadline(
            OsStr::new("nice"),
            &receiver.path,
            &args,
            self.remaining()?,
        )?;
        if !output.status.success() {
            return Err(io::Error::other("incomplete seeded object graph"));
        }
        fs::remove_file(&roots)?;
        self.files.retain(|path| path != &roots);
        Ok(())
    }

    /// A receiver has no published refs yet. Make every imported commit an
    /// explicit root: fsck otherwise skips connectivity of dangling commits.
    /// The staging-only packed refs are removed before the receiver publishes.
    fn write_seed_roots(&mut self, receiver: &Path, path: &Path) -> io::Result<()> {
        let repository = git2::Repository::open_bare(receiver).map_err(io::Error::other)?;
        let objects = repository.odb().map_err(io::Error::other)?;
        let mut file = self.create_file(path)?;
        let mut failure = None;
        let result = objects.foreach(|oid| {
            let step = (|| {
                self.remaining()?;
                let (_, kind) = objects.read_header(*oid).map_err(io::Error::other)?;
                if kind == git2::ObjectType::Commit {
                    writeln!(file, "{oid} refs/build/seed/{oid}")?;
                }
                Ok(())
            })();
            if let Err(error) = step {
                failure = Some(error);
                return false;
            }
            true
        });
        if let Some(error) = failure {
            return Err(error);
        }
        result.map_err(io::Error::other)
    }

    fn loose_directory(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        for entry in fs::read_dir(source)? {
            self.remaining()?;
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (is_hex(&name, 38) || is_hex(&name, 62)) && is_regular(&entry.path()) {
                fs::create_dir_all(destination)?;
                self.link_regular(&entry.path(), &destination.join(name.as_ref()))?;
            }
        }
        Ok(())
    }

    fn packs(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        if !is_directory(source) {
            return Ok(());
        }
        for entry in fs::read_dir(source)? {
            self.remaining()?;
            let source_pack = entry?.path();
            if !is_pack(&source_pack)
                || !is_regular(&source_pack)
                || !is_regular(&source_pack.with_extension("idx"))
            {
                continue;
            }
            let pack = destination.join(
                source_pack
                    .file_name()
                    .ok_or_else(|| io::Error::other("invalid pack path"))?,
            );
            self.link_regular(&source_pack, &pack)?;
            // A private index can be regenerated without writing source data.
            let mut index = open_regular(&source_pack.with_extension("idx"))?;
            let mut copied = self.create_file(&pack.with_extension("idx"))?;
            io::copy(&mut index, &mut copied)?;
            self.create_file(&pack.with_extension("keep"))?
                .write_all(KEEP_MARKER)?;
        }
        Ok(())
    }

    fn link_regular(&mut self, source: &Path, destination: &Path) -> io::Result<()> {
        self.remaining()?;
        (self.link)(source, destination)?;
        self.files.push(destination.into());
        // hard_link does not follow a final symlink. Recheck the new inode
        // before any Git reader sees it, including a source replacement race.
        if !is_regular(destination) {
            return Err(io::Error::other("source object became nonregular"));
        }
        Ok(())
    }

    fn create_file(&mut self, path: &Path) -> io::Result<File> {
        let file = OpenOptions::new().write(true).create_new(true).open(path)?;
        self.files.push(path.into());
        Ok(file)
    }
}

fn is_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_pack(path: &Path) -> bool {
    path.file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.strip_prefix("pack-")?.strip_suffix(".pack"))
        .is_some_and(|name| is_hex(name, 40) || is_hex(name, 64))
}

fn is_regular(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
}

fn is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_dir())
}

fn open_regular(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("nonregular object metadata"));
    }
    Ok(file)
}

/// Once source repacking drops its last link, retaining the pack would pin
/// unrelated history. Release only our exact local marker; GC still protects
/// every receiving/snapshot ref. Call under the receiver maintenance lease.
pub(crate) fn release_unshared_pack_keeps(receiver: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        for entry in
            fs::read_dir(receiver.join("objects/pack")).map_err(|error| error.to_string())?
        {
            let pack = entry.map_err(|error| error.to_string())?.path();
            if !is_pack(&pack) || !is_regular(&pack) {
                continue;
            }
            let metadata = fs::symlink_metadata(&pack).map_err(|error| error.to_string())?;
            if metadata.nlink() != 1 {
                continue;
            }
            let keep = pack.with_extension("keep");
            let Ok(mut file) = open_regular(&keep) else {
                continue;
            };
            let mut marker = Vec::new();
            Read::by_ref(&mut file)
                .take((KEEP_MARKER.len() + 1) as u64)
                .read_to_end(&mut marker)
                .map_err(|error| error.to_string())?;
            if marker == KEEP_MARKER {
                fs::remove_file(keep).map_err(|error| error.to_string())?;
            }
        }
    }
    #[cfg(not(unix))]
    let _ = receiver;
    Ok(())
}
