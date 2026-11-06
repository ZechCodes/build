"""Descriptor-relative source traversal and hashes of the bytes actually copied."""

from contextlib import contextmanager, ExitStack
import hashlib
import os
from pathlib import Path
import stat

from .runtime import BuildctlError, digest


@contextmanager
def open_entry(name, parent_fd, *, source_kind='source', skip_sockets=False):
    if not name or name in {'.', '..'} or '/' in name:
        raise BuildctlError(f'invalid migration {source_kind} component')
    descriptor = None
    try:
        if skip_sockets and stat.S_ISSOCK(os.stat(name, dir_fd=parent_fd, follow_symlinks=False).st_mode):
            yield None
            return
        descriptor = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK | os.O_CLOEXEC,
                             dir_fd=parent_fd)
        mode = os.fstat(descriptor).st_mode
        if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
            raise BuildctlError(f'migration {source_kind} is not a regular file or directory')
        yield descriptor
    except OSError as error:
        raise BuildctlError(f'cannot safely open migration {source_kind}; symlink, unsupported or changed entry') from error
    finally:
        if descriptor is not None:
            os.close(descriptor)


@contextmanager
def open_directory(path, *, source_kind='source'):
    path = Path(path)
    if not path.is_absolute() or '..' in path.parts:
        raise BuildctlError('migration directory must be an absolute canonical path')
    root = os.open('/', os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        with open_relative(root, Path(*path.parts[1:]), source_kind=source_kind) as descriptor:
            if not stat.S_ISDIR(os.fstat(descriptor).st_mode):
                raise BuildctlError('migration directory is not a directory')
            yield descriptor
    finally:
        os.close(root)


@contextmanager
def open_relative(root_fd, relative, *, source_kind='source'):
    relative = Path(relative)
    if relative.is_absolute() or '..' in relative.parts:
        raise BuildctlError(f'migration entry escapes its {source_kind} directory')
    with ExitStack() as stack:
        descriptor = root_fd
        for name in relative.parts:
            if not stat.S_ISDIR(os.fstat(descriptor).st_mode):
                raise BuildctlError(f'migration {source_kind} ancestor is not a directory')
            descriptor = stack.enter_context(open_entry(name, descriptor, source_kind=source_kind))
        yield descriptor


def identity(info):
    return info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns


def copy_file(descriptor, target, *, source_kind='source'):
    original = os.fstat(descriptor)
    signature = hashlib.sha256()
    flags = os.O_RDWR | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC
    with os.fdopen(os.open(target, flags, 0o600), 'w+b') as output:
        while chunk := os.read(descriptor, 1024 * 1024):
            signature.update(chunk)
            output.write(chunk)
        if identity(original) != identity(os.fstat(descriptor)):
            raise BuildctlError(f'migration {source_kind} changed while being copied')
        output.flush()
        output.seek(0)
        if hashlib.file_digest(output, 'sha256').hexdigest() != signature.hexdigest():
            raise BuildctlError('copied migration file hash verification failed')
        os.fchmod(output.fileno(), stat.S_IMODE(original.st_mode))
        os.utime(output.fileno(), ns=(original.st_atime_ns, original.st_mtime_ns))
    return signature.hexdigest()


def copy_open(descriptor, target, *, relative=Path(), exclude=frozenset(),
              source_kind='source', skip_sockets=False):
    """Copy from pinned descriptors; no source pathname is reopened for hashing."""
    info = os.fstat(descriptor)
    if stat.S_ISREG(info.st_mode):
        target.parent.mkdir(parents=True, exist_ok=True)
        return {relative: copy_file(descriptor, target, source_kind=source_kind)}
    if not stat.S_ISDIR(info.st_mode):
        raise BuildctlError(f'migration {source_kind} is not a regular file or directory')
    target.mkdir(parents=True, exist_ok=True)
    hashes = {}
    for name in sorted(os.listdir(descriptor)):
        if name in exclude:
            continue
        with open_entry(name, descriptor, source_kind=source_kind, skip_sockets=skip_sockets) as child:
            if child is not None:
                hashes.update(copy_open(child, target / name, relative=relative / name,
                                        source_kind=source_kind, skip_sockets=skip_sockets))
    os.chmod(target, stat.S_IMODE(info.st_mode))
    os.utime(target, ns=(info.st_atime_ns, info.st_mtime_ns))
    return hashes


def verify_copied(target, hashes, *, attachments_only=False):
    for relative, expected in hashes.items():
        if attachments_only and relative.parts[:2] != ('tasks', 'attachments'):
            continue
        copied = target / relative
        if not copied.is_file() or copied.is_symlink() or digest(copied) != expected:
            raise BuildctlError('copied migration file or attachment hash verification failed')
