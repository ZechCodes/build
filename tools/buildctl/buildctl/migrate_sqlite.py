"""Offline SQLite snapshots from descriptor-pinned, hash-verified private copies.

Cutover callers must stop both bridges before capture. Copy the
database and its present WAL, SHM and rollback-journal files through nofollow
descriptors into a private 0700 directory. Only that private copy is ever opened
by SQLite. Its online backup API includes committed WAL frames, and integrity
checks gate the result. Hot rollback recovery can modify only the private copy.

Stopped capture uses no ctypes, native ABI, external SQLite library, or source
SQLite pathname. Descriptor and file-set checks refuse changes during capture;
these checks supplement, rather than replace, the mandatory stopped-unit gate.
The explicit live-preview exception below permits a read-only pathname open
after descriptor prechecks; it supplies one online snapshot only to dry-run.
"""

from contextlib import contextmanager, ExitStack
import os
from pathlib import Path
import re
import sqlite3
import stat
import tempfile

from .migrate_fs import copy_open, identity, open_directory, open_entry, verify_copied
from .runtime import BuildctlError


def database_files(descriptor, name):
    allowed = {name + suffix for suffix in ('', '-wal', '-shm', '-journal')}
    present = set(os.listdir(descriptor)) & allowed
    if name not in present:
        raise BuildctlError('source SQLite database is missing')
    return present


def copy_database_files(descriptor, directory, name):
    names = database_files(descriptor, name)
    with ExitStack() as stack:
        sources = {}
        for leaf in sorted(names):
            source = stack.enter_context(open_entry(leaf, descriptor))
            info = os.fstat(source)
            if not stat.S_ISREG(info.st_mode):
                raise BuildctlError('SQLite source or sidecar is not a regular file')
            sources[leaf] = source, identity(info)
        hashes = {}
        for leaf, (source, _) in sources.items():
            target = directory / leaf
            hashes.update(copy_open(source, target, relative=Path(leaf)))
            os.chmod(target, 0o600)
        verify_copied(directory, hashes)
        if database_files(descriptor, name) != names:
            raise BuildctlError('SQLite source file set changed during capture')
        for leaf, (source, original) in sources.items():
            current = os.stat(leaf, dir_fd=descriptor, follow_symlinks=False)
            if identity(os.fstat(source)) != original or identity(current) != original:
                raise BuildctlError('SQLite source or sidecar changed during capture')


def integrity_check(connection):
    if connection.execute('PRAGMA integrity_check').fetchall() != [('ok',)]:
        raise BuildctlError('SQLite snapshot integrity check failed')


def _sqlite_backup(source, target, *, live_preview=False):
    # Recovery is allowed only on private copies. The one live-preview path
    # explicitly opens read-only after prechecks and never serves real cutover.
    mode = 'ro' if live_preview else 'rw'
    original = sqlite3.connect(source.as_uri() + '?mode=' + mode, uri=True)
    try:
        if not live_preview:
            integrity_check(original)
        copied = sqlite3.connect(target)
        try:
            original.backup(copied)
            integrity_check(copied)
        finally:
            copied.close()
    finally:
        original.close()


def backup_database_fd(tasks_fd, target, name='build.db'):
    """Capture a stopped source and back up its private copy into a trusted target."""
    if not isinstance(name, str) or name in {'.', '..'} or not re.fullmatch(r'[A-Za-z0-9_.-]+', name):
        raise BuildctlError('SQLite source filename must be a safe basename')
    descriptor = os.dup(tasks_fd)
    try:
        if not stat.S_ISDIR(os.fstat(descriptor).st_mode):
            raise BuildctlError('pinned SQLite source is not a directory')
        with tempfile.TemporaryDirectory(prefix='buildctl-sqlite-source-') as directory:
            private = Path(directory)
            # TemporaryDirectory creates this directory with owner-only mode 0700.
            copy_database_files(descriptor, private, name)
            _sqlite_backup(private / name, target)
    finally:
        os.close(descriptor)


@contextmanager
def connect_fd(tasks_fd, name='build.db'):
    """Yield a read-only connection to a verified snapshot of a stopped source."""
    with tempfile.TemporaryDirectory(prefix='buildctl-sqlite-') as directory:
        snapshot = Path(directory) / 'snapshot.db'
        backup_database_fd(tasks_fd, snapshot, name)
        connection = sqlite3.connect(snapshot.as_uri() + '?mode=ro', uri=True)
        try:
            yield connection
        finally:
            connection.close()


@contextmanager
def connect_path(path):
    """Open all parent components without symlinks, then capture the stopped DB."""
    path = Path(path)
    with open_directory(path.parent) as descriptor:
        with connect_fd(descriptor, path.name) as connection:
            yield connection


@contextmanager
def preview_input(path, *, live=False):
    """Yield one online snapshot only for explicitly authorized live dry-run.

This exception prechecks source leaves using nofollow descriptors, then lets
SQLite reopen the live pathname read-only. It is deliberately not the cutover
copy path: the supplied snapshot is an inventory view, not an apply input.
"""
    path = Path(path)
    if not live:
        yield path
        return
    with open_directory(path.parent) as directory:
        for leaf in sorted(database_files(directory, path.name)):
            with open_entry(leaf, directory) as source:
                if not stat.S_ISREG(os.fstat(source).st_mode):
                    raise BuildctlError('SQLite preview source or sidecar is not a regular file')
    with tempfile.TemporaryDirectory(prefix='buildctl-sqlite-preview-') as directory:
        private = Path(directory)
        # TemporaryDirectory creates this directory with owner-only mode 0700.
        snapshot = private / 'preview.db'
        _sqlite_backup(path, snapshot, live_preview=True)
        yield snapshot
