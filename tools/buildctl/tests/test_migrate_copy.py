"""Copy-time race regressions use only synthetic temporary state."""

from pathlib import Path
import shutil

import pytest

from buildctl.runtime import BuildctlError
import test_migrate
from test_migrate import scratch_fixture, tree_bytes

state = test_migrate.state


def assert_not_published(target, before):
    assert tree_bytes(target) == before
    assert not list(target.parent.glob(target.name + '.backup-*'))


@pytest.mark.parametrize('entry', ['config', 'projects', 'attachment', 'scratch_ancestor'])
def test_source_symlink_swapped_after_inventory_fails_closed(state, tmp_path, monkeypatch, entry):
    migrator, source, target, db, _, _ = state
    if entry == 'scratch_ancestor':
        scratch_fixture(source, db)
    relative = {
        'config': Path('config.json'),
        'projects': Path('projects'),
        'attachment': Path('tasks/attachments/image.bin'),
        'scratch_ancestor': Path('project-scratch'),
    }[entry]
    selected = source / relative
    outside = tmp_path / 'outside'
    outside.mkdir()
    replacement = outside / selected.name
    if selected.is_dir():
        shutil.copytree(selected, replacement)
    else:
        shutil.copy2(selected, replacement)
    (outside / 'sentinel').write_bytes(b'outside state must stay unchanged')
    outside_before = tree_bytes(outside)
    target_before = tree_bytes(target)
    real_copy = migrator.copy_state

    def swap_then_copy(stage, entries):
        selected.rename(selected.with_name(selected.name + '.original'))
        selected.symlink_to(replacement, target_is_directory=replacement.is_dir())
        return real_copy(stage, entries)

    monkeypatch.setattr(migrator, 'copy_state', swap_then_copy)
    with pytest.raises((BuildctlError, OSError)):
        migrator.run(prod_stopped=True)
    assert_not_published(target, target_before)
    assert tree_bytes(outside) == outside_before


def test_replacing_source_cannot_bless_corrupted_attachment_bytes(state, monkeypatch):
    migrator, source, target, _, _, _ = state
    original = source / 'tasks/attachments/image.bin'
    target_before = tree_bytes(target)
    real_copy = migrator.copy_state

    def replace_after_copy(stage, entries):
        copied = real_copy(stage, entries)
        # An atomic source replacement must not change which copied bytes the
        # verifier trusts. Matching a reopened pathname would bless corruption.
        replacement = original.with_name('replacement.bin')
        replacement.write_bytes(b'bytes never read from the copied source fd')
        replacement.replace(original)
        (stage / 'tasks/attachments/image.bin').write_bytes(original.read_bytes())
        return copied

    monkeypatch.setattr(migrator, 'copy_state', replace_after_copy)
    with pytest.raises((BuildctlError, OSError)):
        migrator.run(prod_stopped=True)
    assert_not_published(target, target_before)


def test_private_database_recovery_does_not_publish_original_journal(state):
    migrator, source, target, _, _, _ = state
    (source / 'tasks/build.db-journal').write_bytes(b'\0' * 64)
    migrator.run(prod_stopped=True)
    assert not (target / 'tasks/build.db-journal').exists()
