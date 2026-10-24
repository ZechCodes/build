"""Review remote migration operates only on disposable fixture repositories."""

import json
from pathlib import Path
import sqlite3
from unittest.mock import Mock

import pytest

from buildctl.migrate_git import apply_remotes, plan_remotes
from buildctl.runtime import BuildctlError, Runner


def bindings(tmp_path, records):
    database = tmp_path / 'state.db'
    with sqlite3.connect(database) as connection:
        connection.execute('CREATE TABLE review_branch_bindings (record TEXT)')
        connection.executemany('INSERT INTO review_branch_bindings VALUES (?)',
                               [(json.dumps(record),) for record in records])
    return database


def test_plan_reads_only_database_and_uses_path_boundaries(tmp_path):
    source, target = tmp_path / 'prod', tmp_path / 'dev'
    record = {'working_repository': '/never/access/this',
              'receiving_repository': str(source / 'review-receivers/a.git'),
              'remote_name': 'build-review-123'}
    other = dict(record, receiving_repository=str(source) + '-other/a.git')
    database = bindings(tmp_path, [record, record, other])
    runner = Mock()
    changes = plan_remotes(database, source, target, runner)
    assert changes == [
        {'repository': record['working_repository'], 'key': 'remote.build-review-123.' + suffix,
         'old': record['receiving_repository'],
         'new': str(target / 'review-receivers/a.git')}
        for suffix in ('url', 'pushurl')
    ]
    runner.run.assert_not_called()


@pytest.fixture
def remote(tmp_path):
    runner = Runner()
    repository = tmp_path / 'repository'
    runner.run(['git', 'init', str(repository)])
    source, target = tmp_path / 'prod', tmp_path / 'dev'
    record = {'working_repository': str(repository),
              'receiving_repository': str(source / 'review-receivers/a.git'),
              'remote_name': 'build-review-123'}
    database = bindings(tmp_path, [record])
    changes = plan_remotes(database, source, target, runner)
    for change in changes:
        runner.run(['git', '-C', str(repository), 'config', change['key'], change['old']])
    runner.run(['git', '-C', str(repository), 'config', 'remote.origin.url', str(source / 'unrelated')])
    return runner, repository, changes


def test_apply_and_rollback_restore_both_remote_values_exactly(remote):
    runner, repository, changes = remote
    config = repository / '.git/config'
    original = config.read_bytes()
    apply_remotes(changes, runner)
    for change in changes:
        result = runner.run(['git', '-C', str(repository), 'config', change['key']])
        assert result.stdout.rstrip('\n') == change['new']
    assert runner.run(['git', '-C', str(repository), 'config', 'remote.origin.url']).stdout.strip().endswith('/prod/unrelated')
    apply_remotes(changes, runner, reverse=True)
    assert config.read_bytes() == original
    assert not Path(str(config) + '.lock').exists()


def test_conflicting_value_prevents_any_write(remote):
    runner, repository, changes = remote
    runner.run(['git', '-C', str(repository), 'config', changes[1]['key'], '/other/receiver'])
    config = repository / '.git/config'
    original = config.read_bytes()
    with pytest.raises(BuildctlError, match='changed|expected'):
        apply_remotes(changes, runner)
    assert config.read_bytes() == original
    assert not Path(str(config) + '.lock').exists()


def test_existing_git_lock_is_respected(remote):
    runner, repository, changes = remote
    config = repository / '.git/config'
    lock = Path(str(config) + '.lock')
    lock.write_bytes(b'another writer')
    original = config.read_bytes()
    with pytest.raises(BuildctlError, match='lock'):
        apply_remotes(changes, runner)
    assert config.read_bytes() == original
    assert lock.read_bytes() == b'another writer'


def test_linked_worktree_uses_common_configuration(remote, tmp_path):
    runner, repository, changes = remote
    runner.run(['git', '-C', str(repository), '-c', 'user.name=Fixture',
                '-c', 'user.email=fixture@example.test', 'commit', '--allow-empty', '-m', 'fixture'])
    worktree = tmp_path / 'linked'
    runner.run(['git', '-C', str(repository), 'worktree', 'add', str(worktree)])
    for change in changes:
        change['repository'] = str(worktree)
    apply_remotes(changes, runner)
    assert runner.run(['git', '-C', str(repository), 'config', changes[0]['key']]).stdout.strip() == changes[0]['new']


def test_symlink_config_is_rejected(remote, tmp_path):
    runner, repository, changes = remote
    config = repository / '.git/config'
    external = tmp_path / 'external'
    config.rename(external)
    original = external.read_bytes()
    config.symlink_to(external)
    with pytest.raises(BuildctlError, match='symlink'):
        apply_remotes(changes, runner)
    assert external.read_bytes() == original


def test_all_repositories_are_validated_before_first_write(remote, tmp_path):
    runner, repository, changes = remote
    other = tmp_path / 'other'
    runner.run(['git', 'init', str(other)])
    bad = dict(changes[0], repository=str(other))
    config = repository / '.git/config'
    original = config.read_bytes()
    with pytest.raises(BuildctlError, match='expected'):
        apply_remotes(changes + [bad], runner)
    assert config.read_bytes() == original
    assert not Path(str(config) + '.lock').exists()
    assert not (other / '.git/config.lock').exists()


def test_multi_valued_review_remote_is_refused(remote):
    runner, repository, changes = remote
    runner.run(['git', '-C', str(repository), 'config', '--add', changes[0]['key'], changes[0]['old']])
    config = repository / '.git/config'
    original = config.read_bytes()
    with pytest.raises(BuildctlError, match='expected'):
        apply_remotes(changes, runner)
    assert config.read_bytes() == original


def test_database_without_review_bindings_has_empty_plan(tmp_path):
    database = tmp_path / 'state.db'
    sqlite3.connect(database).close()
    runner = Mock()
    assert plan_remotes(database, tmp_path / 'prod', tmp_path / 'dev', runner) == []
    runner.run.assert_not_called()


@pytest.mark.parametrize("reverse", [False, True])
def test_later_replace_failure_restores_previously_written_configs(remote, tmp_path, monkeypatch, reverse):
    import os

    runner, repository, changes = remote
    other = tmp_path / 'other'
    runner.run(['git', 'clone', '--bare', str(repository), str(other)])
    for change in changes:
        runner.run(['git', '--git-dir', str(other), 'config', change['key'], change['old']])
    others = [dict(change, repository=str(other)) for change in changes]
    configs = [repository / '.git/config', other / 'config']
    if reverse:
        apply_remotes(changes + others, runner)
    originals = {config: config.read_bytes() for config in configs}
    original_replace = os.replace
    writes = []

    def fail_second_replace(source, destination):
        writes.append(destination)
        if len(writes) == 2:
            raise OSError('injected replacement failure')
        original_replace(source, destination)

    monkeypatch.setattr('buildctl.migrate_git.os.replace', fail_second_replace)
    with pytest.raises(BuildctlError, match='restored'):
        apply_remotes(changes + others, runner, reverse=reverse)
    for config in configs:
        assert config.read_bytes() == originals[config]
        assert not Path(str(config) + '.lock').exists()
        assert not list(config.parent.glob('config.migrate-*'))


@pytest.mark.parametrize('already_old', [1, 2])
def test_reverse_recovers_mixed_or_already_restored_values(remote, already_old):
    runner, repository, changes = remote
    config = repository / '.git/config'
    original = config.read_bytes()
    apply_remotes(changes, runner)
    for change in changes[:already_old]:
        runner.run(['git', '-C', str(repository), 'config', change['key'], change['old']])
    apply_remotes(changes, runner, reverse=True)
    assert config.read_bytes() == original


def test_reverse_refuses_unrelated_remote_destination(remote):
    runner, repository, changes = remote
    apply_remotes(changes, runner)
    runner.run(['git', '-C', str(repository), 'config', changes[1]['key'], '/third/destination'])
    config = repository / '.git/config'
    original = config.read_bytes()
    with pytest.raises(BuildctlError, match='expected'):
        apply_remotes(changes, runner, reverse=True)
    assert config.read_bytes() == original


def test_plan_maps_literal_prod_alias_with_exact_boundary(tmp_path):
    source, target = tmp_path / 'prod', tmp_path / 'dev'
    record = {'working_repository': '/never/access/shared',
              'receiving_repository': '~/.build/review-receivers/a.git',
              'remote_name': 'build-review-123'}
    other = dict(record, receiving_repository='~/.build-other/review-receivers/a.git')
    runner = Mock()
    changes = plan_remotes(bindings(tmp_path, [record, other]), source, target, runner)
    assert len(changes) == 2
    assert all(change['old'] == record['receiving_repository'] for change in changes)
    assert all(change['new'] == str(target / 'review-receivers/a.git') for change in changes)
    runner.run.assert_not_called()
