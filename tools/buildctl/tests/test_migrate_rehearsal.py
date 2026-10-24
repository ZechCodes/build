"""Opt-in rehearsal against an explicitly supplied, offline snapshot."""

import json
import os
from pathlib import Path
import tempfile

import pytest

from buildctl.migrate import Migrator


def test_real_snapshot_rehearsal():
    raw = os.environ.get('BUILDCTL_REHEARSAL_ROOT')
    if not raw:
        pytest.skip('set BUILDCTL_REHEARSAL_ROOT explicitly to authorize a snapshot rehearsal')
    root = Path(raw)
    snapshot = root / 'prod-snapshot'
    target = Path(tempfile.mkdtemp(prefix='verified-dev-', dir=root))
    migrator = Migrator(snapshot, target, source_prefix=Path('/home/zech/.build'),
                        projects_dir=Path('/home/zech/Projects'),
                        worktrees=Path('/home/zech/Projects/.build-worktrees'),
                        states=lambda: {'prod': 'inactive', 'dev': 'inactive'}, rehearsal=True)
    dry = migrator.run(dry_run=True)
    result = migrator.run(prod_stopped=True)
    assert result['verified']
    assert result['tables'] == dry['tables']
    assert result['rewrites'] == dry['rewrites']
    assert (target / 'tasks/resume-roster.live.json').is_file()
    assert not (target / 'identity.json').exists()
    result['target'] = str(target)
    report = root / 'rehearsal-verify.json'
    report.write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
    restored = migrator.rollback(Path(result['backup']))
    assert list(target.iterdir()) == []
    print('ROLLBACK ' + json.dumps(restored))
