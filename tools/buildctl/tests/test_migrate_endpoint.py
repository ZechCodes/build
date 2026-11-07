"""HTTP endpoint generations belong to the target device, not imported tasks."""

from pathlib import Path
import sqlite3

import pytest

import test_migrate
from test_migrate import tree_bytes, write_json

state = test_migrate.state
ENDPOINT_FILES = ('http-endpoint.json', 'http-endpoint.json.lock')


@pytest.mark.parametrize('source_has_endpoint', [False, True])
@pytest.mark.parametrize('target_files', [ENDPOINT_FILES, (), ENDPOINT_FILES[1:]])
def test_migration_retains_only_target_device_endpoint(state, source_has_endpoint, target_files):
    migrator, source, target, _, _, _ = state
    if source_has_endpoint:
        write_json(source / 'tasks/http-endpoint.json', {'epoch': 'source-generation'})
        (source / 'tasks/http-endpoint.json.lock').write_bytes(b'source lock')
    for name in target_files:
        path = target / 'tasks' / name
        path.parent.mkdir(exist_ok=True)
        if name.endswith('.json'):
            # Even structured-looking values in this device-owned file are opaque
            # to path rewriting. Whitespace and permissions survive verbatim.
            path.write_text('{ "epoch": "target-generation", "path": "' + str(source) + '" }\n')
        else:
            path.write_bytes(b'target lock')
        path.chmod(0o640)
    original_endpoint = {name: (target / 'tasks' / name).read_bytes() for name in target_files}
    write_json(target / 'tasks/old-task.json', {'old': True})
    before_target, before_source = tree_bytes(target), tree_bytes(source)

    result = migrator.run(force=True, prod_stopped=True)

    for name in ENDPOINT_FILES:
        path = target / 'tasks' / name
        if name in original_endpoint:
            assert path.read_bytes() == original_endpoint[name]
            assert path.stat().st_mode & 0o777 == 0o640
        else:
            assert not path.exists()
    assert not (target / 'tasks/old-task.json').exists()
    assert (target / 'tasks/attachments/image.bin').read_bytes() == (source / 'tasks/attachments/image.bin').read_bytes()
    assert tree_bytes(source) == before_source
    assert tree_bytes(Path(result['backup'])) == before_target
    migrator.rollback(Path(result['backup']))
    assert tree_bytes(target) == before_target


def test_second_forced_migration_preserves_current_target_endpoint(state):
    migrator, source, target, source_db, _, _ = state
    write_json(target / 'tasks/http-endpoint.json', {'epoch': 'target-first'})
    (target / 'tasks/http-endpoint.json.lock').write_bytes(b'target first lock')
    write_json(source / 'tasks/http-endpoint.json', {'epoch': 'source-first'})
    (source / 'tasks/http-endpoint.json.lock').write_bytes(b'source first lock')
    write_json(source / 'tasks/imported-task.json', {'revision': 1})
    before_first = tree_bytes(target)
    first = migrator.run(force=True, prod_stopped=True)

    # Target registration and source task data advance independently between
    # cutovers. A repeated forced run must keep the target's current generation.
    endpoint = {
        'http-endpoint.json': (b'{ "epoch": "target-second", "enabled": false }\n', 0o640),
        'http-endpoint.json.lock': (b'target second lock', 0o600),
    }
    for name, (content, mode) in endpoint.items():
        path = target / 'tasks' / name
        path.write_bytes(content)
        path.chmod(mode)
    write_json(target / 'tasks/target-only.json', {'discard_on_import': True})
    before_second = tree_bytes(target)
    write_json(source / 'tasks/http-endpoint.json', {'epoch': 'source-second'})
    (source / 'tasks/http-endpoint.json.lock').write_bytes(b'source second lock')
    write_json(source / 'tasks/imported-task.json', {'revision': 2})
    source_attachment = source / 'tasks/attachments/image.bin'
    source_attachment.write_bytes(b'new attachment for second cutover')
    source_db.execute('UPDATE extra_state SET content=?', (b'second cutover database',))
    source_db.commit()
    before_source = tree_bytes(source)

    second = migrator.run(force=True, prod_stopped=True)

    for name, (content, mode) in endpoint.items():
        path = target / 'tasks' / name
        assert path.read_bytes() == content
        assert path.stat().st_mode & 0o777 == mode
    assert (target / 'tasks/imported-task.json').read_bytes() == (source / 'tasks/imported-task.json').read_bytes()
    assert (target / 'tasks/attachments/image.bin').read_bytes() == source_attachment.read_bytes()
    with sqlite3.connect(target / 'tasks/build.db') as copied:
        assert copied.execute('SELECT content FROM extra_state').fetchone()[0] == b'second cutover database'
    assert not (target / 'tasks/target-only.json').exists()
    assert tree_bytes(source) == before_source
    assert second['backup'] != first['backup']
    assert tree_bytes(Path(second['backup'])) == before_second

    migrator.rollback(Path(second['backup']))
    assert tree_bytes(target) == before_second
    for name, (_, mode) in endpoint.items():
        assert (target / 'tasks' / name).stat().st_mode & 0o777 == mode
    migrator.rollback(Path(first['backup']))
    assert tree_bytes(target) == before_first
