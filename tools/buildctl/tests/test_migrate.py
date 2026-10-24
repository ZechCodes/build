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
