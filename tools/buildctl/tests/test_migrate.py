"""Migration rehearsals use only synthetic state under pytest's temporary root."""

import hashlib
import json
from pathlib import Path
import sqlite3

import pytest

from buildctl.migrate import Migrator
from buildctl.runtime import BuildctlError


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value))


def tree_bytes(root):
    # SQLite mode=ro may update WAL shared-memory read marks. DB/WAL and all
    # persistent state bytes must remain unchanged; SHM is transient coordination.
    return {
        str(path.relative_to(root)): path.read_bytes()
        for path in root.rglob('*') if path.is_file() and path.name != 'build.db-shm'
    }


@pytest.fixture
def state(tmp_path):
    source, target = tmp_path / 'prod', tmp_path / 'dev'
    projects = tmp_path / 'Projects'
    worktrees = projects / '.build-worktrees'
    workspace = worktrees / 'workspaces' / 'one'
    workspace.mkdir(parents=True)
    source.mkdir()
    target.mkdir()
    write_json(source / 'identity.json', {'secret': 'prod-private'})
    write_json(target / 'identity.json', {'secret': 'dev-private'})
    attachment = source / 'tasks/attachments/image.bin'
    attachment.parent.mkdir(parents=True)
    attachment.write_bytes(bytes(range(256)))
    write_json(source / 'config.json', {
        'projects_dir': str(projects), 'projects': [str(projects / 'repo')],
        'project_agent': {'harness_session_id': 'session-kept'},
        'nested': {'state_path': str(source / 'projects/proj.json')},
    })
    write_json(source / 'projects/proj.json', {'workspace': str(workspace)})
    write_json(source / 'provider-recovery/one.json', {
        'session_id': 'session-kept', 'path': str(source / 'projects/proj.json'),
    })
    write_json(source / 'tasks/harness-inventory.json', {'session_id': 'session-kept'})
    write_json(source / 'tasks/bridge-update-status.json', {'status': 'idle'})
    receiver = source / 'review-receivers/one.git'
    receiver.mkdir(parents=True)
    (receiver / 'config').write_text('[remote "origin"]\n\turl = ' + str(source / 'projects/repo') + '\n')
    write_json(source / 'log/ignored.json', {'ignore': True})
    db = sqlite3.connect(source / 'tasks/build.db')
    db.execute('PRAGMA journal_mode=WAL')
    db.execute('PRAGMA wal_autocheckpoint=0')
    for table in ('tasks', 'agents', 'conversations', 'workspaces'):
        db.execute(f'CREATE TABLE {table} (id TEXT PRIMARY KEY, record TEXT NOT NULL)')
        record = {'id': table + '-1', 'session_id': 'session-kept',
                  'path': str(workspace), 'attachment': str(attachment),
                  'boundary': str(source) + '-other/file',
                  'body': str(attachment), 'content': str(attachment),
                  'text': 'A historic mention of ' + str(attachment)}
        db.execute(f'INSERT INTO {table} VALUES (?,?)', (table + '-1', json.dumps(record)))
    db.execute('CREATE TABLE conversation_attachment_uploads (path TEXT PRIMARY KEY)')
    db.execute('INSERT INTO conversation_attachment_uploads VALUES (?)', (str(attachment),))
    db.execute('CREATE TABLE extra_state (id INTEGER PRIMARY KEY, content BLOB)')
    db.execute('INSERT INTO extra_state VALUES (1, ?)', (b'opaque\x00bytes',))
    db.commit()
    # Keep the writer open: committed records are deliberately still in the WAL.
    assert Path(str(source / 'tasks/build.db') + '-wal').stat().st_size > 0
    migrator = Migrator(source, target, projects_dir=projects, worktrees=worktrees,
                        states=lambda: {'prod': 'inactive', 'dev': 'inactive'}, rehearsal=True)
    yield migrator, source, target, db, projects, worktrees
    db.close()


def test_online_backup_preserves_rows_and_rewrites_state(state):
    migrator, source, target, original, projects, _ = state
    identity = (target / 'identity.json').read_bytes()
    result = migrator.run(prod_stopped=True)
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        tables = [row[0] for row in original.execute("SELECT name FROM sqlite_master WHERE type='table'")]
        for table in tables:
            assert copied.execute(f'SELECT COUNT(*) FROM {table}').fetchone() == original.execute(f'SELECT COUNT(*) FROM {table}').fetchone()
            assert result['tables'][table] == 1
        for table in ('tasks', 'agents', 'conversations', 'workspaces'):
            record = json.loads(copied.execute(f'SELECT record FROM {table}').fetchone()[0])
            assert record['id'] == table + '-1'
            assert record['session_id'] == 'session-kept'
            assert record['path'].startswith(str(projects))
            assert record['attachment'] == str(target / 'tasks/attachments/image.bin')
            assert record['boundary'] == str(source) + '-other/file'
            assert record['body'] == str(source / 'tasks/attachments/image.bin')
            assert record['content'] == str(source / 'tasks/attachments/image.bin')
            assert record['text'] == 'A historic mention of ' + str(source / 'tasks/attachments/image.bin')
        assert copied.execute('SELECT content FROM extra_state').fetchone()[0] == b'opaque\x00bytes'
        assert copied.execute('SELECT path FROM conversation_attachment_uploads').fetchone()[0] == str(target / 'tasks/attachments/image.bin')
    config = json.loads((target / 'config.json').read_text())
    assert config['projects_dir'] == str(projects)
    assert config['projects'] == [str(projects / 'repo')]
    assert config['nested']['state_path'] == str(target / 'projects/proj.json')
    recovery = json.loads((target / 'provider-recovery/one.json').read_text())
    assert recovery == {'session_id': 'session-kept', 'path': str(target / 'projects/proj.json')}
    assert str(target / 'projects/repo') in (target / 'review-receivers/one.git/config').read_text()
    assert (target / 'identity.json').read_bytes() == identity
    assert not (target / 'log/ignored.json').exists()
    assert result['attachments'] == 1
    assert result['rewritten_values'] > 0
    assert hashlib.sha256((target / 'tasks/attachments/image.bin').read_bytes()).digest() == hashlib.sha256((source / 'tasks/attachments/image.bin').read_bytes()).digest()


def test_dry_run_is_read_only_even_with_active_prod(state):
    migrator, source, target, _, _, _ = state
    migrator.states = lambda: {'prod': 'active', 'dev': 'inactive'}
    before_source, before_target = tree_bytes(source), tree_bytes(target)
    result = migrator.run(dry_run=True)
    assert result['tables']['tasks'] == 1
    assert result['attachments'] == 1
    assert result['rewrites']['tasks']['record'] > 0
    assert result['rewrites']['conversation_attachment_uploads']['path'] == 1
    assert tree_bytes(source) == before_source
    assert tree_bytes(target) == before_target
    assert not list(target.parent.glob(target.name + '*backup*'))


def test_requires_explicit_prod_stopped_assertion(state):
    migrator, _, target, _, _, _ = state
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='prod.stopped|stopped'):
        migrator.run()
    assert tree_bytes(target) == before


@pytest.mark.parametrize('unit,status', [('prod', 'active'), ('dev', 'active'), ('prod', 'unknown'), ('dev', 'activating')])
def test_refuses_unsafe_service_states(state, unit, status):
    migrator, _, target, _, _, _ = state
    migrator.states = lambda: {'prod': 'inactive', 'dev': 'inactive', unit: status}
    before = tree_bytes(target)
    with pytest.raises(BuildctlError):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_fresh_target_refusal_force_backup_and_rollback(state):
    migrator, _, target, _, _, _ = state
    write_json(target / 'config.json', {'old': True})
    (target / 'old-file').write_bytes(b'dev-original')
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='fresh|force|existing'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before
    result = migrator.run(prod_stopped=True, force=True)
    backup = Path(result['backup'])
    assert backup.is_dir()
    assert tree_bytes(backup) == before
    assert json.loads((target / 'config.json').read_text()) != {'old': True}
    migrator.rollback(backup)
    assert tree_bytes(target) == before


@pytest.mark.parametrize('location', ['source', 'target', 'attachment', 'parent'])
def test_rejects_symlinks_without_following_them(state, tmp_path, location):
    migrator, source, target, _, _, _ = state
    external = tmp_path / 'external'
    external.mkdir()
    (external / 'sentinel').write_text('untouched')
    if location == 'source':
        migrator.source = tmp_path / 'source-link'
        migrator.source.symlink_to(source, target_is_directory=True)
    elif location == 'target':
        migrator.target = tmp_path / 'target-link'
        migrator.target.symlink_to(target, target_is_directory=True)
    elif location == 'parent':
        link = tmp_path / 'parent-link'
        link.symlink_to(external, target_is_directory=True)
        migrator.target = link / 'new-root'
    else:
        (source / 'tasks/attachments/unsafe').symlink_to(external, target_is_directory=True)
    with pytest.raises(BuildctlError, match='symlink'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(external) == {'sentinel': b'untouched'}


def test_source_identity_is_never_imported_into_unpaired_target(state):
    migrator, _, target, _, _, _ = state
    (target / 'identity.json').unlink()
    migrator.run(prod_stopped=True)
    assert not (target / 'identity.json').exists()


def test_missing_worktree_refuses_strict_apply(state):
    migrator, _, target, _, _, worktrees = state
    (worktrees / 'workspaces/one').rmdir()
    migrator.rehearsal = False
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='worktree|workspace|missing'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def scratch_fixture(source, db):
    """Only a structured DB reference authorizes importing a scratch checkout."""
    scratch = source / 'project-scratch/Build-a1b2c3d4e5f6'
    scratch.mkdir(parents=True)
    (scratch / 'tracked.txt').write_bytes(b'checkout content')
    write_json(scratch / '.build/session.json', {
        'checkout': str(scratch), 'session_id': 'scratch-session',
        'body': str(scratch),
    })
    record = {
        'id': 'workspaces-1', 'path': str(scratch),
        'threads': [{'sessions': [{'checkout': str(scratch), 'id': 'provider-session'}]}],
    }
    db.execute('UPDATE workspaces SET record=?', (json.dumps(record),))
    db.commit()
    return scratch


def test_imports_only_db_referenced_scratch_checkouts(state):
    migrator, source, target, db, _, _ = state
    scratch = scratch_fixture(source, db)
    unrelated = source / 'project-scratch/Build-000000000000'
    unrelated.mkdir()
    (unrelated / 'unrelated').write_text('not imported')
    orchestrator = source / 'project-scratch/migrate-rehearsal'
    orchestrator.mkdir()
    (orchestrator / 'private').write_text('not imported')
    # Even an exact scratch pathname inside historical prose grants no copy.
    db.execute('UPDATE conversations SET record=?', (json.dumps({'body': str(unrelated)}),))
    db.commit()
    migrator.run(prod_stopped=True)
    copied = target / 'project-scratch' / scratch.name
    assert (copied / 'tracked.txt').read_bytes() == b'checkout content'
    assert sorted(path.name for path in (target / 'project-scratch').iterdir()) == [scratch.name]
    sidecar = json.loads((copied / '.build/session.json').read_text())
    assert sidecar['checkout'] == str(copied)
    assert sidecar['session_id'] == 'scratch-session'
    assert sidecar['body'] == str(scratch)
    with sqlite3.connect(target / 'tasks/build.db') as migrated:
        record = json.loads(migrated.execute('SELECT record FROM workspaces').fetchone()[0])
        assert record['path'] == str(copied)
        assert record['threads'][0]['sessions'][0] == {'checkout': str(copied), 'id': 'provider-session'}
        prose = json.loads(migrated.execute('SELECT record FROM conversations').fetchone()[0])
        assert prose['body'] == str(unrelated)


def test_missing_referenced_scratch_is_a_hard_failure_even_in_rehearsal(state):
    migrator, source, target, db, _, _ = state
    scratch = source / 'project-scratch/Build-a1b2c3d4e5f6'
    db.execute('UPDATE workspaces SET record=?', (json.dumps({'path': str(scratch)}),))
    db.commit()
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='scratch|missing'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_unrecognized_scratch_directory_is_not_imported(state):
    migrator, source, target, db, _, _ = state
    scratch = source / 'project-scratch/orchestrator'
    scratch.mkdir(parents=True)
    (scratch / 'private').write_text('not imported')
    db.execute('UPDATE workspaces SET record=?', (json.dumps({'path': str(scratch)}),))
    db.commit()
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='scratch|unsupported|invalid|name'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_literal_home_paths_are_rewritten_only_in_structured_values(state):
    migrator, _, target, db, _, _ = state
    record = {
        'attachment': '~/.build/tasks/attachments/image.bin',
        'nested': [{'checkout_path': '~/.build/projects'}],
        'text': {'attachment': '~/.build/tasks/attachments/image.bin'},
        'body': '~/.build/tasks/attachments/image.bin',
        'content': 'Historical ~/.build/projects mention',
        'path': '~/.build-other/unchanged',
    }
    db.execute('UPDATE tasks SET record=?', (json.dumps(record),))
    db.commit()
    migrator.run(prod_stopped=True)
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        result = json.loads(copied.execute('SELECT record FROM tasks').fetchone()[0])
    assert result['attachment'] == str(target / 'tasks/attachments/image.bin')
    assert result['nested'][0]['checkout_path'] == str(target / 'projects')
    for key in ('body', 'content', 'text', 'path'):
        assert result[key] == record[key]


def test_root_and_cwd_references_are_mapped_without_changing_prose(state):
    migrator, source, target, db, _, _ = state
    location = str(source / 'projects')
    record = {'root': location, 'session': {'cwd': location},
              'body': location, 'text': {'root': location, 'cwd': location}}
    db.execute('UPDATE agents SET record=?', (json.dumps(record),))
    db.commit()
    migrator.run(prod_stopped=True)
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        actual = json.loads(copied.execute('SELECT record FROM agents').fetchone()[0])
    assert actual['root'] == str(target / 'projects')
    assert actual['session']['cwd'] == str(target / 'projects')
    assert actual['body'] == record['body']
    assert actual['text'] == record['text']


def test_scratch_descendant_reference_does_not_authorize_checkout_copy(state):
    migrator, source, target, db, _, _ = state
    scratch = source / 'project-scratch/Build-a1b2c3d4e5f6'
    descendant = scratch / 'nested'
    descendant.mkdir(parents=True)
    (scratch / 'private').write_text('unselected checkout data')
    db.execute('UPDATE workspaces SET record=?', (json.dumps({'path': str(descendant)}),))
    db.commit()
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='scratch|missing|reference'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_unselected_orchestrator_symlink_is_not_traversed(state, tmp_path):
    migrator, source, target, _, _, _ = state
    orchestrator = source / 'project-scratch/migrate-rehearsal'
    orchestrator.mkdir(parents=True)
    outside = tmp_path / 'orchestrator-private'
    outside.mkdir()
    (outside / 'sentinel').write_text('unread and unchanged')
    (orchestrator / 'external').symlink_to(outside, target_is_directory=True)
    migrator.run(prod_stopped=True)
    assert not (target / 'project-scratch/migrate-rehearsal').exists()
    assert tree_bytes(outside) == {'sentinel': b'unread and unchanged'}


@pytest.mark.parametrize('reference', ['record_attachment', 'upload_path'])
def test_missing_db_attachment_reference_fails_even_if_existing_files_hash_match(state, reference):
    migrator, source, target, db, _, _ = state
    missing = str(source / 'tasks/attachments/missing.bin')
    if reference == 'record_attachment':
        db.execute('UPDATE conversations SET record=?', (json.dumps({'attachment': missing}),))
    else:
        db.execute('INSERT INTO conversation_attachment_uploads VALUES (?)', (missing,))
    db.commit()
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='attachment|missing|reference'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_failed_git_publish_retains_recoverable_backup(state, monkeypatch):
    migrator, _, target, _, _, _ = state
    migrator.rehearsal = False
    before = tree_bytes(target)
    with monkeypatch.context() as patch:
        def fail(*args, **kwargs):
            raise BuildctlError('injected shared config failure')
        patch.setattr('buildctl.migrate.apply_remotes', fail)
        with pytest.raises(BuildctlError, match='backup|recover'):
            migrator.run(prod_stopped=True)
    backups = list(target.parent.glob(target.name + '.backup-*'))
    backup = next(path for path in backups if path.is_dir())
    assert tree_bytes(backup) == before
    migrator.rollback(backup)
    assert tree_bytes(target) == before


def test_bridge_operations_share_migration_lock(tmp_path):
    from buildctl.bridge import management_lock
    from buildctl.migrate import migration_lock
    from buildctl.registry import load_registry
    registry = Path(__file__).parents[1] / 'environments.toml'
    env = load_registry(registry, tmp_path)['dev']
    with migration_lock(env.state_root):
        with pytest.raises(BuildctlError, match='migration'):
            with management_lock(env):
                pass


def add_operation(db, status, delivery):
    db.execute('CREATE TABLE operations (operation_id TEXT PRIMARY KEY, status TEXT, delivery TEXT)')
    db.execute('INSERT INTO operations VALUES (?, ?, ?)', ('operation-1', status, json.dumps(delivery)))
    db.commit()


@pytest.mark.parametrize('status', ['claimed', 'delivered'])
def test_terminal_delivery_roots_are_separate_from_other_references(state, status):
    from buildctl.migrate_store import scan_database

    migrator, source, _, db, _, _ = state
    root = str(source / 'project-scratch/Build-012345abcdef')
    attachment = str(source / 'tasks/attachments/image.bin')
    add_operation(db, status, {'root': root, 'attachment': attachment})
    # A regular row referencing the same path must retain its independent ref.
    db.execute('UPDATE agents SET record=?', (json.dumps({'cwd': root}),))
    db.commit()
    scanned = scan_database(source / 'tasks/build.db', migrator.paths)
    assert scanned['terminal_operation_roots'] == {root}
    assert ('root', root) not in scanned['references']
    assert ('cwd', root) in scanned['references']
    assert ('attachment', attachment) in scanned['references']
    assert scanned['tables']['operations'] == 1
    assert scanned['rewrites']['operations']['delivery'] == 1


@pytest.mark.parametrize('status', ['queued', 'uncertain', 'unknown', None, '', 'future-status'])
def test_nonterminal_or_unknown_operation_roots_remain_required(state, status):
    from buildctl.migrate_store import scan_database

    migrator, source, _, db, _, _ = state
    root = str(source / 'project-scratch/Build-012345abcdef')
    add_operation(db, status, {'root': root})
    scanned = scan_database(source / 'tasks/build.db', migrator.paths)
    assert scanned['terminal_operation_roots'] == set()
    assert ('root', root) in scanned['references']
    with pytest.raises(BuildctlError, match='scratch|missing'):
        migrator.run(prod_stopped=True)


def test_missing_terminal_delivery_root_warns_without_dropping_the_receipt(state):
    migrator, source, target, db, _, _ = state
    root = str(source / 'project-scratch/Build-012345abcdef')
    add_operation(db, 'delivered', {'root': root, 'owner_id': 'project-1'})
    result = migrator.run(prod_stopped=True)
    assert root in str(result['historical_operation_roots'])
    assert not (target / 'project-scratch/Build-012345abcdef').exists()
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        operation = copied.execute('SELECT operation_id,status,delivery FROM operations').fetchone()
    assert operation[:2] == ('operation-1', 'delivered')
    assert json.loads(operation[2]) == {'root': str(target / 'project-scratch/Build-012345abcdef'), 'owner_id': 'project-1'}


def test_existing_terminal_delivery_root_is_imported_and_verified(state):
    migrator, source, target, db, _, _ = state
    root = source / 'project-scratch/Build-012345abcdef'
    root.mkdir(parents=True)
    (root / 'checkout-data').write_bytes(b'imported referenced checkout')
    add_operation(db, 'delivered', {'root': str(root)})
    result = migrator.run(prod_stopped=True)
    assert (target / 'project-scratch/Build-012345abcdef/checkout-data').read_bytes() == b'imported referenced checkout'
    assert str(root) not in str(result['historical_operation_roots'])


def test_json_looking_operation_error_remains_opaque(state):
    from buildctl.migrate_store import scan_database

    migrator, source, target, db, _, _ = state
    add_operation(db, 'delivered', {'owner_id': 'project-1'})
    db.execute('ALTER TABLE operations ADD COLUMN execution_error TEXT')
    prose = json.dumps({'root': str(source / 'project-scratch/Build-012345abcdef')})
    db.execute('UPDATE operations SET execution_error=?', (prose,))
    db.commit()
    scanned = scan_database(source / 'tasks/build.db', migrator.paths)
    assert ('root', json.loads(prose)['root']) not in scanned['references']
    assert not scanned['terminal_operation_roots']
    migrator.run(prod_stopped=True)
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        assert copied.execute('SELECT execution_error FROM operations').fetchone()[0] == prose


def test_terminal_classification_does_not_drop_same_root_from_regular_rows(state):
    from buildctl.migrate_store import scan_database

    migrator, source, _, db, _, _ = state
    root = str(source / 'project-scratch/Build-012345abcdef')
    add_operation(db, 'delivered', {'root': root})
    db.execute('UPDATE agents SET record=?', (json.dumps({'root': root}),))
    db.commit()
    scanned = scan_database(source / 'tasks/build.db', migrator.paths)
    assert scanned['terminal_operation_roots'] == {root}
    assert ('root', root) in scanned['references']
    with pytest.raises(BuildctlError, match='scratch|missing'):
        migrator.run(prod_stopped=True)


def test_pi_jsonl_session_paths_move_without_rewriting_conversation_text(state):
    migrator, source, target, _, _, _ = state
    session = source / 'harness/pi/sessions/agent-one/session.jsonl'
    session.parent.mkdir(parents=True)
    header = {'type': 'session', 'cwd': str(source / 'projects'), 'id': 'provider-session'}
    message = {'type': 'message', 'content': [{'text': str(source / 'projects')}]}
    session.write_text(json.dumps(header) + '\n' + json.dumps(message) + '\n')
    migrator.run(prod_stopped=True)
    lines = (target / session.relative_to(source)).read_text().splitlines()
    assert json.loads(lines[0]) == dict(header, cwd=str(target / 'projects'))
    assert json.loads(lines[1]) == message


def test_schema_identifiers_are_quoted_and_values_bound(state):
    migrator, source, target, db, _, _ = state
    name = 'state"; DROP TABLE agents; --'
    quoted = '"' + name.replace('"', '""') + '"'
    db.execute(f'CREATE TABLE {quoted} (id INTEGER PRIMARY KEY, record TEXT)')
    db.execute(f'INSERT INTO {quoted} VALUES (?, ?)', (1, json.dumps({'path': str(source / 'projects')})))
    db.commit()
    migrator.run(prod_stopped=True)
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        assert json.loads(copied.execute(f'SELECT record FROM {quoted}').fetchone()[0])['path'] == str(target / 'projects')
        assert copied.execute('SELECT COUNT(*) FROM agents').fetchone()[0] == 1


def test_finished_run_missing_checkout_is_reported_by_lifecycle(state):
    migrator, _, _, db, _, worktrees = state
    missing = str(worktrees / 'removed-workspace')
    db.execute('CREATE TABLE implementations (id TEXT PRIMARY KEY, record TEXT)')
    db.execute('INSERT INTO implementations VALUES (?, ?)', ('old', json.dumps({'id': 'old', 'state': 'Archived', 'worktree_path': missing})))
    db.commit()
    result = migrator.run(prod_stopped=True)
    assert result['historical_worktrees']['finished_runs']['count'] == 1
    assert missing in result['historical_worktrees']['finished_runs']['sample']


def test_live_workspace_manifest_keeps_a_missing_finished_path_mandatory(state):
    migrator, _, target, db, _, worktrees = state
    missing = str(worktrees / 'removed-workspace')
    db.execute('CREATE TABLE implementations (id TEXT PRIMARY KEY, record TEXT)')
    db.execute('INSERT INTO implementations VALUES (?, ?)', ('old', json.dumps({'id': 'old', 'state': 'Archived', 'worktree_path': missing})))
    db.commit()
    write_json(worktrees / 'workspaces/proj-1/live/.build-workspace.json', {'id': 'live', 'root': missing, 'directories': [], 'status': 'ready'})
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='worktree|missing'):
        migrator.run(prod_stopped=True)
    assert tree_bytes(target) == before


def test_historical_missing_review_repository_does_not_schedule_git_writes(state):
    migrator, source, _, db, _, worktrees = state
    missing = str(worktrees / 'removed-review')
    db.execute('CREATE TABLE tracker_tasks (id TEXT PRIMARY KEY, record TEXT)')
    db.execute('INSERT INTO tracker_tasks VALUES (?, ?)', ('done', json.dumps({'id': 'done', 'state': 'closed'})))
    db.execute('CREATE TABLE review_branch_bindings (id TEXT PRIMARY KEY, task_id TEXT, record TEXT)')
    binding = {'task_id': 'done', 'working_repository': missing, 'receiving_repository': str(source / 'review-receivers/one.git'), 'remote_name': 'build-review'}
    db.execute('INSERT INTO review_branch_bindings VALUES (?, ?, ?)', ('binding', 'done', json.dumps(binding)))
    db.commit()
    result = migrator.run(prod_stopped=True)
    assert result['shared_git_edits'] == 0
    assert result['historical_git_edits']['count'] == 2


def test_missing_git_remote_does_not_prevent_state_rollback(state):
    import subprocess

    migrator, source, target, db, projects, _ = state
    migrator.rehearsal = False
    repository = projects / 'review-repository'
    subprocess.run(['git', 'init', str(repository)], check=True, capture_output=True)
    db.execute('CREATE TABLE review_branch_bindings (id TEXT PRIMARY KEY, record TEXT)')
    binding = {'working_repository': str(repository), 'receiving_repository': str(source / 'review-receivers/one.git'), 'remote_name': 'build-review'}
    db.execute('INSERT INTO review_branch_bindings VALUES (?, ?)', ('binding', json.dumps(binding)))
    db.commit()
    before = tree_bytes(target)
    with pytest.raises(BuildctlError, match='rollback'):
        migrator.run(prod_stopped=True)
    backup = next(path for path in target.parent.glob(target.name + '.backup-*') if path.is_dir())
    migrator.rollback(backup)
    assert tree_bytes(target) == before
