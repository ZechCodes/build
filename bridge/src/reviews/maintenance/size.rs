//! Conservative allocated bytes: a hardlink still held elsewhere frees none.
use super::*;
use std::collections::BTreeSet;
use std::fs;

pub(super) fn allocated(path: &Path) -> Result<u64, String> {
    let mut seen = BTreeSet::new();
    walk(path, &mut seen)
}

fn walk(path: &Path, seen: &mut BTreeSet<(u64, u64)>) -> Result<u64, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    if metadata.is_dir() {
        return fs::read_dir(path)
            .map_err(|error| error.to_string())?
            .try_fold(0u64, |total, entry| {
                Ok(total.saturating_add(walk(
                    &entry.map_err(|error| error.to_string())?.path(),
                    seen,
                )?))
            });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(
            if metadata.is_file()
                && metadata.nlink() == 1
                && seen.insert((metadata.dev(), metadata.ino()))
            {
                metadata.blocks().saturating_mul(512)
            } else {
                0
            },
        )
    }
    #[cfg(not(unix))]
    {
        let _ = seen;
        // Physical allocation/link ownership is unknown: do not claim savings.
        Ok(0)
    }
}
