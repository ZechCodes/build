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
