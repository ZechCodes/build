"""Plan review remote cutovers without inspecting shared repositories."""

from contextlib import ExitStack, contextmanager
import json
import os
from pathlib import Path
import re
import sqlite3
import tempfile

from .runtime import BuildctlError


def plan_remotes(database: Path, source_prefix: Path, target_prefix: Path, runner):
    """Read bindings only; shared repositories are opened exclusively at apply time."""
    changes = {}
    with sqlite3.connect(f'{database.as_uri()}?mode=ro', uri=True) as connection:
        exists = connection.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='review_branch_bindings'"
        ).fetchone()
        if not exists:
            return []
        for (raw,) in connection.execute('SELECT record FROM review_branch_bindings'):
            for change in _binding_changes(json.loads(raw), source_prefix, target_prefix):
                identity = (change['repository'], change['key'])
                if identity in changes and changes[identity] != change:
                    raise BuildctlError('conflicting review remote bindings')
                changes[identity] = change
    return list(changes.values())


def _binding_changes(binding, source, target):
    receiver = Path(binding['receiving_repository'])
    if not receiver.is_relative_to(source):
        return []
    name = binding['remote_name']
    if not re.fullmatch(r'[A-Za-z0-9-]+', name):
        raise BuildctlError('invalid review remote name')
    return [
        {'repository': binding['working_repository'], 'key': f'remote.{name}.{suffix}',
         'old': str(receiver), 'new': str(target / receiver.relative_to(source))}
        for suffix in ('url', 'pushurl')
    ]


def _reject_symlinks(path):
    for component in (path, *path.parents):
        if component.is_symlink():
            raise BuildctlError('review configuration path contains a symlink')


def _configuration(repository, runner):
    repository = Path(repository)
    _reject_symlinks(repository)
    result = runner.run(['git', '-C', str(repository), 'rev-parse',
                         '--path-format=absolute', '--git-common-dir'])
    config = Path(result.stdout.strip()) / 'config'
    _reject_symlinks(config)
    return config


@contextmanager
def _locked(config):
    lock = Path(str(config) + '.lock')
    try:
        descriptor = os.open(lock, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    except FileExistsError as error:
        raise BuildctlError('review repository configuration is locked') from error
    try:
        os.close(descriptor)
        _reject_symlinks(config)
        yield
    finally:
        lock.unlink()


def _config_command(runner, config, *arguments, check=True):
    return runner.run(['git', 'config', '--no-includes', '--file', str(config),
                       *arguments], check=check)


def _validate(config, changes, runner, reverse):
    for change in changes:
        if not re.fullmatch(r'remote\.[A-Za-z0-9-]+\.(url|pushurl)', change['key']):
            raise BuildctlError('invalid review configuration key')
        result = _config_command(runner, config, '--null', '--get-all', change['key'], check=False)
        expected = change['new'] if reverse else change['old']
        if result.returncode or result.stdout != expected + '\0':
            raise BuildctlError('review remote changed from expected migration value')


@contextmanager
def _staged(config, changes, runner, reverse):
    descriptor, name = tempfile.mkstemp(prefix='config.migrate-', dir=config.parent)
    staged = Path(name)
    try:
        with os.fdopen(descriptor, 'wb') as stream:
            stream.write(config.read_bytes())
        os.chmod(staged, config.stat().st_mode & 0o777)
        for change in changes:
            value = change['old'] if reverse else change['new']
            _config_command(runner, staged, '--replace-all', change['key'], value)
        with staged.open('rb') as stream:
            os.fsync(stream.fileno())
        yield staged
    finally:
        staged.unlink(missing_ok=True)


def apply_remotes(changes, runner, *, reverse=False):
    """Atomically update each shared config, refusing stale plans or Git locks.

    All destinations are locked and checked before any configuration is changed.
    Rollback uses the same plan with old/new expectations exchanged.
    """
    grouped = {}
    repositories = {}
    for change in changes:
        repository = change['repository']
        if repository not in repositories:
            repositories[repository] = _configuration(repository, runner)
        config = repositories[repository]
        grouped.setdefault(config, {})[change['key']] = change
    with ExitStack() as stack:
        for config in sorted(grouped):
            stack.enter_context(_locked(config))
            _validate(config, grouped[config].values(), runner, reverse)
        staged = [(config, stack.enter_context(_staged(
            config, records.values(), runner, reverse)))
            for config, records in grouped.items()]
        for config, temporary in staged:
            os.replace(temporary, config)
