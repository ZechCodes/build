"""Deleted workspace and receiver regressions use synthetic roots only."""
import json
import sqlite3

import pytest

import test_migrate
from test_migrate import tree_bytes, write_json
from buildctl.runtime import BuildctlError

state = test_migrate.state


def insert_records(db, table, records):
    statements = {
        'tracker_tasks': (
            'CREATE TABLE IF NOT EXISTS tracker_tasks (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO tracker_tasks VALUES (?, ?, ?)'),
        'tracker_events': (
            'CREATE TABLE IF NOT EXISTS tracker_events (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO tracker_events VALUES (?, ?, ?)'),
        'reviews': (
            'CREATE TABLE IF NOT EXISTS reviews (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO reviews VALUES (?, ?, ?)'),
        'review_branch_bindings': (
            'CREATE TABLE IF NOT EXISTS review_branch_bindings (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO review_branch_bindings VALUES (?, ?, ?)'),
        'review_snapshots': (
            'CREATE TABLE IF NOT EXISTS review_snapshots (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO review_snapshots VALUES (?, ?, ?)'),
        'review_openings': (
            'CREATE TABLE IF NOT EXISTS review_openings (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO review_openings VALUES (?, ?, ?)'),
        'review_merge_intents': (
            'CREATE TABLE IF NOT EXISTS review_merge_intents (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO review_merge_intents VALUES (?, ?, ?)'),
        'review_actions': (
            'CREATE TABLE IF NOT EXISTS review_actions (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)',
            'INSERT INTO review_actions VALUES (?, ?, ?)'),
    }
    create, insert = statements[table]
    db.execute(create)
    for n, record in enumerate(records):
        db.execute(insert, (str(n), record.get('task_id'), json.dumps(record)))
    db.commit()

def test_deleted_workspace_on_open_task_is_history_even_with_stale_db_row(state):
    migrator, _, _, db, _, worktrees = state
    missing = str(worktrees / 'workspaces/proj-1/deleted')
    insert_records(db, 'tracker_tasks', [{'id': 'task', 'state': 'open',
                   'links': {'workspace_ids': ['deleted']}, 'workspace': {'root': missing}}])
    db.execute('INSERT INTO workspaces VALUES (?, ?)',
               ('deleted', json.dumps({'id': 'deleted', 'root': missing})))
    insert_records(db, 'review_snapshots', [{'task_id': 'task', 'directories': [
        {'path': missing + '/Build', 'source_path': missing + '/Build'}]}])
    db.execute('UPDATE conversations SET record=?',
               (json.dumps({'checkout_path': missing + '/prior'}),))
    db.commit()
    preview = migrator.run(dry_run=True)
    assert preview['historical_worktrees']['deleted_workspaces'] == {
        'count': 3, 'sample': [missing, missing + '/Build', missing + '/prior']}
    result = migrator.run(prod_stopped=True)
    assert result['historical_worktrees'] == preview['historical_worktrees']
    assert result['verified'] is True


@pytest.mark.parametrize('registered', [False, True])
def test_missing_checkout_in_existing_or_registered_workspace_still_fails(state, registered):
    migrator, _, target, db, _, worktrees = state
    root = worktrees / 'workspaces/proj-1/live'
    if registered:
        write_json(worktrees / 'workspaces/proj-1/registry/.build-workspace.json',
                   {'id': 'live', 'root': str(root), 'directories': [{'path': str(root / 'Build')}]})
    else:
        root.mkdir(parents=True)
    insert_records(db, 'tracker_tasks', [{'id': 'task', 'state': 'open', 'root': str(root / 'Build')}])
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='worktree|missing'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def receiver_records(state, *, status='open', live=False):
    migrator, source, _, db, _, worktrees = state
    workspace = worktrees / 'workspaces/proj-1/review'
    repository = workspace / 'Build'
    receiver = str(source / 'review-receivers/missing.git')
    if live:
        repository.mkdir(parents=True)
        write_json(workspace / '.build-workspace.json', {'id': 'review-ws', 'root': str(workspace),
                   'directories': [{'path': str(repository)}]})
    insert_records(db, 'reviews', [{'task_id': 'task', 'workspace_id': 'review-ws',
                   'pull_request': {'status': status}}])
    insert_records(db, 'review_branch_bindings', [{'task_id': 'task', 'working_repository': str(repository),
                   'receiving_repository': receiver, 'remote_name': 'build-review'}])
    # A scalar SQL reference and snapshot Git metadata must agree with the binding.
    db.execute('ALTER TABLE review_branch_bindings ADD COLUMN receiving_repository TEXT')
    db.execute('UPDATE review_branch_bindings SET receiving_repository=?', (receiver,))
    insert_records(db, 'review_snapshots', [{'task_id': 'task', 'directories': [{'common_git_dir': receiver}]}])
    db.commit()
    return receiver


@pytest.mark.parametrize('status', ['open', 'changes_requested', 'approved', 'rejected', 'closed', 'merged'])
def test_missing_receiver_without_live_workspace_is_reported_and_rewritten(state, status):
    migrator, source, target, _, _, _ = state
    receiver = receiver_records(state, status=status)
    preview = migrator.run(dry_run=True)
    assert preview['historical_worktrees']['review_receivers'] == {'count': 1, 'sample': [receiver]}
    assert 'review_receiver_policy' in preview
    result = migrator.run(prod_stopped=True)
    assert result['verified'] is True
    with sqlite3.connect(target / 'tasks/build.db') as db:
        raw, scalar = db.execute('SELECT record, receiving_repository FROM review_branch_bindings').fetchone()
    assert json.loads(raw)['receiving_repository'] == scalar == receiver.replace(str(source), str(target), 1)


@pytest.mark.parametrize('status', ['open', 'changes_requested', 'approved', 'rejected', 'closed'])
def test_live_workspace_receiver_remains_required_including_retained_terminal_prs(state, status):
    migrator, _, target, _, _, _ = state
    receiver_records(state, status=status, live=True)
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_merged_receiver_is_history_unless_live_manifest_directly_needs_it(state):
    migrator, _, _, _, _, worktrees = state
    receiver = receiver_records(state, status='merged', live=True)
    assert migrator.run(dry_run=True)['historical_worktrees']['review_receivers']['count'] == 1
    write_json(worktrees / 'workspaces/proj-1/review/.build-workspace.json',
               {'id': 'review-ws', 'root': str(worktrees / 'workspaces/proj-1/review'),
                'directories': [{'path': str(worktrees / 'workspaces/proj-1/review/Build'),
                                 'common_git_dir': receiver}]})
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


def test_receiver_shared_by_historical_and_live_reviews_still_fails(state):
    migrator, _, _, db, _, _ = state
    receiver = receiver_records(state, live=True)
    db.execute('INSERT INTO reviews VALUES (?, ?, ?)', ('other', 'old', json.dumps({
        'task_id': 'old', 'workspace_id': 'removed', 'pull_request': {'status': 'merged'},
        'bindings': [{'receiving_repository': receiver}]})))
    db.commit()
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


@pytest.mark.parametrize('suffix', ['missing.git', 'missing.git/../../tasks/missing'])
def test_receiver_history_never_excuses_missing_attachments(state, suffix):
    migrator, source, _, db, _, _ = state
    missing = str(source / 'review-receivers' / suffix)
    db.execute('UPDATE conversations SET record=?', (json.dumps({'attachment': missing}),))
    db.commit()
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


def test_existing_receiver_never_excuses_missing_child(state):
    migrator, source, _, db, _, _ = state
    missing = str(source / 'review-receivers/one.git/missing')
    insert_records(db, 'review_snapshots', [{'common_git_dir': missing}])
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


def test_live_receiver_requirement_overrides_closed_task_history(state):
    migrator, _, _, db, _, _ = state
    receiver_records(state, live=True)
    # Only common_git_dir remains, which otherwise inherits closed-task history.
    db.execute('DELETE FROM review_branch_bindings')
    insert_records(db, 'tracker_tasks', [{'id': 'task', 'state': 'closed'}])
    db.commit()
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


@pytest.mark.parametrize('opening_state', ['preparing', 'failed', 'interrupted'])
def test_live_pending_opening_retains_receiver_even_after_prior_merged_pr(state, opening_state):
    migrator, _, _, db, _, _ = state
    receiver = receiver_records(state, status='merged', live=True)
    insert_records(db, 'review_openings', [{'task_id': 'task', 'state': opening_state,
        'request': {'workspace_id': 'review-ws'}, 'bindings': [{'receiving_repository': receiver}]}])
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


def test_live_pending_capture_retains_merged_receiver(state):
    migrator, _, _, db, _, _ = state
    receiver_records(state, status='merged', live=True)
    db.execute('CREATE TABLE review_sync_candidates (task_id TEXT, snapshot_id TEXT)')
    db.execute('INSERT INTO review_sync_candidates VALUES (?, ?)', ('task', 'snapshot'))
    db.commit()
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


@pytest.mark.parametrize('merge_state', ['running', 'failed', 'interrupted', 'future-state'])
def test_live_unsettled_merge_retains_merged_receiver(state, merge_state):
    migrator, _, _, db, _, _ = state
    receiver_records(state, status='merged', live=True)
    insert_records(db, 'review_merge_intents', [{'task_id': 'task', 'state': merge_state}])
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)


def test_succeeded_merge_with_unpublished_push_retains_merged_receiver(state):
    migrator, _, _, db, _, _ = state
    receiver_records(state, status='merged', live=True)
    insert_records(db, 'review_merge_intents', [{'task_id': 'task', 'state': 'succeeded',
        'action_ids': ['push'], 'request': {'sources': [
            {'directory_id': 'dir', 'push': {'remote': 'origin', 'branch': 'main'}}]}}])
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)
    insert_records(db, 'review_actions', [{'id': 'push', 'task_id': 'task', 'directory_id': 'dir',
        'steps': [{'kind': 'push', 'status': 'succeeded', 'remote': 'origin', 'branch': 'main'}]}])
    assert migrator.run(dry_run=True)['historical_worktrees']['review_receivers']['count'] == 1


def test_live_receiver_requirement_overrides_old_reclaim_evidence(state):
    migrator, _, _, db, _, _ = state
    receiver = receiver_records(state, live=True)
    db.execute('DELETE FROM review_branch_bindings')
    insert_records(db, 'tracker_events', [{'kind': 'workspace_reclaimed',
        'payload': {'workspace_id': 'old', 'root': receiver}}])
    db.commit()
    with pytest.raises(BuildctlError, match='missing'):
        migrator.run(dry_run=True)
