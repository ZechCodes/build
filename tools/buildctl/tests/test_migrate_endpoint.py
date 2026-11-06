"""HTTP endpoint generations belong to the target device, not imported tasks."""

from pathlib import Path

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
