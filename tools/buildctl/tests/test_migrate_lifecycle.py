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
