"""Device-vault boundaries use synthetic fixtures, never a live key or value."""

import os
from pathlib import Path

import pytest

import test_migrate
from buildctl.runtime import BuildctlError

state = test_migrate.state


def test_source_vault_is_excluded_from_migration_and_rollback(state):
    migrator, source, target, _, _, _ = state
    vault = source / 'secrets'
    vault.mkdir(mode=0o700)
    key = b'synthetic vault key fixture only'
    blob = b'synthetic encrypted blob fixture only'
    (vault / 'vault.key').write_bytes(key)
    (vault / 'sec_00000000-0000-4000-8000-000000000001').write_bytes(blob)
    result = migrator.run(prod_stopped=True)
    assert 'secrets' not in result['entries']
    assert not (target / 'secrets').exists()
    assert not (Path(result['backup']) / 'secrets').exists()
    assert (vault / 'vault.key').read_bytes() == key
    assert (vault / 'sec_00000000-0000-4000-8000-000000000001').read_bytes() == blob


def test_force_cannot_copy_or_backup_an_existing_target_vault(state):
    migrator, _, target, _, _, _ = state
    vault = target / 'secrets'
    vault.mkdir(mode=0o700)
    (vault / 'vault.key').write_bytes(b'synthetic target key fixture only')
    before = test_migrate.tree_bytes(target)
    with pytest.raises(BuildctlError, match='vault'):
        migrator.run(force=True, prod_stopped=True)
    assert test_migrate.tree_bytes(target) == before
    assert not list(target.parent.glob(target.name + '.backup-*'))
    assert not list(target.parent.glob('.' + target.name + '.stage-*'))


@pytest.mark.parametrize('kind', ['regular', 'fifo', 'broken_symlink'])
def test_target_vault_entry_is_refused_without_opening_it(state, kind):
    migrator, _, target, _, _, _ = state
    vault = target / 'secrets'
    if kind == 'regular':
        vault.write_bytes(b'synthetic corrupt vault fixture')
    elif kind == 'fifo':
        os.mkfifo(vault)
    else:
        vault.symlink_to(target.parent / 'missing-vault')
    with pytest.raises(BuildctlError, match='vault'):
        migrator.run(force=True, prod_stopped=True)
    assert os.path.lexists(vault)
    assert not list(target.parent.glob(target.name + '.backup-*'))


def test_excluded_source_vault_is_never_traversed(state):
    migrator, source, target, _, _, _ = state
    vault = source / 'secrets'
    vault.mkdir(mode=0o700)
    os.mkfifo(vault / 'vault.key')
    (vault / 'unfollowed').symlink_to(source.parent / 'outside-vault')
    result = migrator.run(prod_stopped=True)
    assert result['verified']
    assert not (target / 'secrets').exists()


@pytest.mark.parametrize('relative', ['secrets', 'secrets/vault.key'])
def test_copy_entry_cannot_override_vault_exclusion(state, tmp_path, relative):
    migrator, source, _, _, _, _ = state
    vault = source / 'secrets'
    vault.mkdir(mode=0o700)
    (vault / 'vault.key').write_bytes(b'synthetic vault key fixture only')
    stage = tmp_path / 'isolated-stage'
    stage.mkdir()
    with pytest.raises(BuildctlError, match='vault'):
        migrator.copy_state(stage, [Path(relative)])
    assert list(stage.iterdir()) == []


def test_allowlisted_directory_cannot_alias_source_vault(state):
    migrator, source, target, _, _, _ = state
    vault = source / 'secrets'
    vault.mkdir(mode=0o700)
    (vault / 'vault.key').write_bytes(b'synthetic vault key fixture only')
    projects = source / 'projects'
    projects.rename(source / 'original-projects')
    projects.symlink_to(vault, target_is_directory=True)
    before = test_migrate.tree_bytes(target)
    with pytest.raises(BuildctlError, match='symlink'):
        migrator.run(prod_stopped=True)
    assert test_migrate.tree_bytes(target) == before


def vault_entry(root, kind):
    path = root / 'secrets'
    if kind == 'directory':
        path.mkdir(mode=0o700)
        (path / 'vault.key').write_bytes(b'synthetic rollback vault key fixture only')
    elif kind == 'regular':
        path.write_bytes(b'synthetic rollback vault entry fixture only')
    elif kind == 'fifo':
        os.mkfifo(path)
    else:
        path.symlink_to(root.parent / 'synthetic-missing-vault')


def nofollow_snapshot(path):
    """Read synthetic regular files only; never open a FIFO or follow a link."""
    import stat
    info = path.lstat()
    identity = (info.st_ino, info.st_mode)
    if stat.S_ISLNK(info.st_mode):
        return identity, os.readlink(path)
    if stat.S_ISREG(info.st_mode):
        return identity, path.read_bytes()
    if stat.S_ISDIR(info.st_mode):
        return identity, {child.name: nofollow_snapshot(child) for child in sorted(path.iterdir())}
    return identity, None


def rollback_fixture(state, monkeypatch):
    import json
    import buildctl.migrate as migration
    migrator, _, target, _, _, _ = state
    result = migrator.run(prod_stopped=True)
    backup = Path(result['backup'])
    metadata = Path(str(backup) + '.json')
    manifest = json.loads(metadata.read_text())
    # Synthetic migration uses rehearsal, but rollback must exercise the real
    # remote-edit ordering. A sentinel replaces every actual Git operation.
    manifest.update(rehearsal=False, git_status='committing')
    metadata.write_text(json.dumps(manifest))
    remote = target.parent / 'synthetic-review-remote.config'
    remote.write_bytes(b'synthetic migrated remote value')
    calls, traversed = [], []

    def edit_remotes(*args, **kwargs):
        calls.append((args, kwargs))
        remote.write_bytes(b'synthetic unexpectedly restored remote value')

    real_no_symlinks = migration.no_symlinks

    def watch_traversal(path, *, tree=False):
        if tree and os.path.lexists(Path(path) / 'secrets'):
            traversed.append(path)
        return real_no_symlinks(path, tree=tree)

    monkeypatch.setattr(migration, 'apply_remotes', edit_remotes)
    monkeypatch.setattr(migration, 'no_symlinks', watch_traversal)
    return migrator, target, backup, metadata, remote, calls, traversed


def rollback_snapshot(target, backup, metadata, remote):
    return nofollow_snapshot(target), nofollow_snapshot(backup), metadata.read_bytes(), remote.read_bytes()


def assert_rollback_refused_without_effects(fixture, before):
    _, target, backup, metadata, remote, calls, traversed = fixture
    assert rollback_snapshot(target, backup, metadata, remote) == before
    assert calls == []
    assert traversed == []
    assert not list(target.parent.glob(target.name + '.retired-*'))


@pytest.mark.parametrize('location', ['target', 'backup'])
@pytest.mark.parametrize('kind', ['directory', 'regular', 'fifo', 'broken_symlink'])
def test_rollback_refuses_either_vault_before_traversal_remotes_or_renames(state, monkeypatch,
                                                                       location, kind):
    fixture = rollback_fixture(state, monkeypatch)
    migrator, target, backup, metadata, remote, _, _ = fixture
    vault_entry(target if location == 'target' else backup, kind)
    before = rollback_snapshot(target, backup, metadata, remote)
    with pytest.raises(BuildctlError, match='vault'):
        migrator.rollback(backup)
    assert_rollback_refused_without_effects(fixture, before)


@pytest.mark.parametrize('location', ['target', 'backup'])
@pytest.mark.parametrize('kind', ['directory', 'regular', 'fifo', 'broken_symlink'])
def test_restore_rechecks_late_vault_before_remotes_or_renames(state, monkeypatch, location, kind):
    fixture = rollback_fixture(state, monkeypatch)
    migrator, target, backup, metadata, remote, _, _ = fixture
    real_restore = migrator.restore
    before = []

    def insert_vault_then_restore(*args):
        vault_entry(target if location == 'target' else backup, kind)
        before.append(rollback_snapshot(target, backup, metadata, remote))
        return real_restore(*args)

    monkeypatch.setattr(migrator, 'restore', insert_vault_then_restore)
    with pytest.raises(BuildctlError, match='vault'):
        migrator.rollback(backup)
    assert len(before) == 1
    assert_rollback_refused_without_effects(fixture, before[0])
