"""Lifecycle evidence, rather than checkout names, governs missing paths."""
import json
import sqlite3

from buildctl.migrate_lifecycle import classify


def database(tmp_path, records):
    path = tmp_path / 'build.db'
    with sqlite3.connect(path) as db:
        for table, values in records.items():
            db.execute(f'CREATE TABLE {table} (id TEXT, record TEXT)')
            db.executemany(f'INSERT INTO {table} VALUES (?, ?)',
                           [(str(n), json.dumps(value)) for n, value in enumerate(values)])
    return path


def test_finished_run_and_ended_lineage_are_separate_history(tmp_path):
    path = database(tmp_path, {
        'implementations': [{'id': 'run', 'state': 'Merged', 'worktree_path': '/checkout/old',
                             'project_path': '/repository'}],
        'conversations': [{'sessions': [{'checkout': '/checkout/prior', 'ended_at': 'yesterday'},
                                       {'checkout': '/checkout/current', 'ended_at': None}]}],
    })
    result = classify(path)
    assert ('worktree_path', '/checkout/old') in result['historical']['finished_runs']
    assert ('checkout', '/checkout/prior') in result['historical']['session_lineage']
    assert ('checkout', '/checkout/current') in result['required']
    assert ('project_path', '/repository') in result['required']


def test_live_manifest_wins_over_finished_run_and_reclaim_event(tmp_path):
    path = database(tmp_path, {
        'implementations': [{'state': 'Archived', 'worktree_path': '/checkout/live'}],
        'tracker_events': [{'kind': 'workspace_reclaimed',
                            'payload': {'workspace_id': 'ws', 'root': '/checkout/live'}}],
        'conversations': [{'sessions': [{'checkout': '/checkout/live', 'ended_at': 'then'}]}],
    })
    result = classify(path, [{'id': 'ws', 'root': '/checkout/live', 'directories': []}])
    assert ('worktree_path', '/checkout/live') in result['required']
    assert ('checkout', '/checkout/live') in result['required']
    assert not any(result['historical'].values())


def test_reclaim_event_classifies_linked_session_and_worktree(tmp_path):
    path = database(tmp_path, {
        'tracker_events': [{'kind': 'workspace_reclaimed',
                            'payload': {'workspace_id': 'ws', 'root': '/checkout/gone'}}],
        'implementations': [{'workspace_id': 'ws', 'worktree_path': '/checkout/gone'}],
        'conversations': [{'sessions': [{'entity_id': 'ws', 'checkout': '/checkout/gone'}]}],
    })
    result = classify(path)
    assert ('worktree_path', '/checkout/gone') in result['historical']['reclaimed_workspaces']
    assert ('checkout', '/checkout/gone') in result['historical']['reclaimed_workspaces']


def test_closed_tasks_archives_unknown_and_reclaimable_are_distinct(tmp_path):
    path = database(tmp_path, {
        'tracker_tasks': [{'state': 'closed', 'workspace_path': '/checkout/closed'}],
        'archived_worktrees': [{'worktree_path': '/checkout/archived'}],
        'implementations': [{'state': 'Failed', 'worktree_path': '/checkout/unknown'}],
        'workspaces': [{'id': 'eligible', 'root': '/checkout/eligible',
                        'lifecycle': {'reclaimable': True}}],
    })
    result = classify(path)
    assert ('workspace_path', '/checkout/closed') in result['historical']['closed_tasks']
    assert ('worktree_path', '/checkout/archived') in result['historical']['archived_worktrees']
    assert ('worktree_path', '/checkout/unknown') in result['required']
    assert ('root', '/checkout/eligible') in result['required']


def test_only_finished_tasks_make_conversation_worktree_historical(tmp_path):
    path = database(tmp_path, {
        'tracker_tasks': [
            {'id': 'closed', 'state': 'closed', 'links': {'conversation_ids': ['old', 'shared']}},
            {'id': 'open', 'state': 'open', 'status': 'doing', 'links': {'conversation_ids': ['shared']}},
        ],
        'implementations': [{'id': 'old', 'state': 'Review', 'worktree_path': '/old'},
                            {'id': 'shared', 'state': 'Review', 'worktree_path': '/shared'}],
        'conversations': [{'sessions': [{'entity_id': 'old', 'checkout': '/old'}]}],
        'review_branch_bindings': [{'task_id': 'closed', 'working_repository': '/old'}],
    })
    result = classify(path)
    assert ('worktree_path', '/old') in result['historical']['closed_tasks']
    assert ('checkout', '/old') in result['historical']['closed_tasks']
    assert ('working_repository', '/old') in result['historical']['closed_tasks']
    assert ('worktree_path', '/shared') in result['required']


def test_terminal_operations_do_not_veto_history_and_review_records_join_tasks(tmp_path):
    path = database(tmp_path, {
        'tracker_tasks': [{'id': 'done', 'status': 'done'}],
        'reviews': [{'task_id': 'done', 'workspace_id': 'ws', 'working_repository': '/gone'}],
        'review_snapshots': [{'task_id': 'done', 'working_repository': '/gone'}],
        'conversations': [{'sessions': [{'checkout': '/gone', 'ended_at': 'then'}]}],
    })
    with sqlite3.connect(path) as db:
        db.execute('CREATE TABLE operations (status TEXT, delivery TEXT)')
        db.execute('INSERT INTO operations VALUES (?, ?)',
                   ('delivered', json.dumps({'root': '/gone'})))
    result = classify(path)
    assert ('checkout', '/gone') in result['historical']['session_lineage']
    assert ('working_repository', '/gone') in result['historical']['closed_tasks']
    assert ('root', '/gone') not in result['required']


def test_agents_inherit_terminal_owner_and_cancelled_deliveries_are_history(tmp_path):
    path = database(tmp_path, {
        'implementations': [{'id': 'run', 'state': 'Archived', 'worktree_path': '/gone'}],
        'agents': [{'owner_id': 'run', 'thread': {'sessions': [{'checkout': '/gone'}]}}],
        'tracker_delivery_intents': [{'state': 'cancelled', 'admitted': {'path': '/cancelled'}}],
    })
    result = classify(path)
    assert ('checkout', '/gone') in result['historical']['finished_runs']
    assert ('path', '/cancelled') in result['historical']['cancelled_deliveries']


def test_review_binding_joins_reclaimed_workspace_without_path_heuristics(tmp_path):
    path = database(tmp_path, {
        'tracker_events': [{'kind': 'workspace_reclaimed', 'payload': {'workspace_id': 'ws'}}],
        'reviews': [{'task_id': 'task', 'workspace_id': 'ws'}],
        'review_branch_bindings': [{'task_id': 'task', 'working_repository': '/arbitrary/place'}],
    })
    result = classify(path)
    assert ('working_repository', '/arbitrary/place') in result['historical']['reclaimed_workspaces']


def add_operations(path, values):
    with sqlite3.connect(path) as db:
        db.execute('CREATE TABLE operations (operation_id TEXT, status TEXT, entity_id TEXT, delivery TEXT)')
        db.executemany('INSERT INTO operations VALUES (?, ?, ?, ?)',
                       [(identity, status, target, json.dumps({'root': root}))
                        for identity, status, target, root in values])


def test_stale_operations_use_target_lifecycle_and_keep_unknown_targets_required(tmp_path):
    path = database(tmp_path, {
        'implementations': [{'id': 'finished', 'state': 'Merged', 'worktree_path': '/finished'},
                            {'id': 'live', 'state': 'Review', 'worktree_path': '/live'}],
    })
    add_operations(path, [('old', 'uncertain', 'finished', '/finished'),
                          ('current', 'queued', 'live', '/live'),
                          ('unknown', 'queued', 'absent', '/unknown')])
    result = classify(path)
    assert result['undeliverable_stale_operations'] == [
        {'id': 'old', 'target': 'finished', 'status': 'uncertain', 'root': '/finished'}]
    assert ('root', '/finished') in result['historical']['undeliverable_stale_operations']
    assert ('root', '/live') in result['required']
    assert ('root', '/unknown') in result['required']


def test_reclaimed_workspace_links_prove_stale_target_but_live_run_wins(tmp_path):
    path = database(tmp_path, {
        'tracker_events': [{'kind': 'workspace_reclaimed', 'payload': {'workspace_id': 'ws'}}],
        'tracker_tasks': [{'status': 'done', 'links': {'workspace_ids': ['ws'],
                                                    'conversation_ids': ['gone', 'live']}}],
        'implementations': [{'id': 'live', 'state': 'Review', 'worktree_path': '/live'}],
    })
    add_operations(path, [('stale', 'queued', 'gone', '/arbitrary'),
                          ('current', 'uncertain', 'live', '/live')])
    result = classify(path)
    assert [r['id'] for r in result['undeliverable_stale_operations']] == ['stale']
    assert ('root', '/live') in result['required']


def test_failed_materialized_delivery_needs_authoritative_deleted_workspace_evidence(tmp_path):
    path = database(tmp_path, {'tracker_delivery_intents': [
        {'id': 'failed', 'state': 'failed', 'reservation': {'materialized': True,
         'workspace': {'id': 'removed', 'root': '/removed', 'directories': [{'path': '/removed/repo'}]}}},
    ]})
    assert ('root', '/removed') in classify(path)['required']
    result = classify(path, [])
    assert result['undeliverable_stale_deliveries'] == [
        {'id': 'failed', 'target': 'removed', 'status': 'failed', 'root': '/removed'}]
    assert ('path', '/removed/repo') in result['historical']['undeliverable_stale_deliveries']
    live = classify(path, [{'id': 'removed', 'root': '/removed'}])
    assert ('root', '/removed') in live['required']
    assert not live['undeliverable_stale_deliveries']


def test_historical_review_directory_source_path_does_not_veto_path_history(tmp_path):
    path = database(tmp_path, {
        'tracker_tasks': [{'id': 'task', 'status': 'done'}],
        'review_snapshots': [{'task_id': 'task', 'directories': [
            {'path': '/old/checkout', 'source_path': '/old/checkout'}]}],
    })
    result = classify(path)
    assert ('path', '/old/checkout') in result['historical']['closed_tasks']
    assert ('source_path', '/old/checkout') in result['historical']['closed_tasks']


def test_review_git_metadata_is_historical_unless_within_live_workspace(tmp_path):
    path = database(tmp_path, {
        'tracker_tasks': [{'id': 'task', 'status': 'done'}],
        'review_snapshots': [{'task_id': 'task', 'common_git_dir': '/old/repo/.git'}],
    })
    assert ('common_git_dir', '/old/repo/.git') in classify(path)['historical']['closed_tasks']
    live = classify(path, [{'id': 'ws', 'root': '/old', 'directories': [{'path': '/old/repo'}]}])
    assert ('common_git_dir', '/old/repo/.git') in live['required']
