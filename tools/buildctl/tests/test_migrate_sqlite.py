"""Pinned-directory SQLite snapshots, exercised only against disposable databases."""

from contextlib import contextmanager
import os
import sqlite3
import stat
import subprocess
import sys

import pytest

from buildctl import migrate_sqlite
from buildctl.migrate_sqlite import backup_database_fd, connect_fd, connect_path, preview_input
from buildctl.runtime import BuildctlError


@pytest.fixture
def wal_database(tmp_path):
    tasks = tmp_path / 'tasks'
    tasks.mkdir()
    writer = sqlite3.connect(tasks / 'build.db')
    writer.execute('PRAGMA journal_mode=WAL')
    writer.execute('PRAGMA wal_autocheckpoint=0')
    writer.execute('CREATE TABLE records (value TEXT)')
    writer.execute("INSERT INTO records VALUES ('committed in WAL')")
    writer.commit()
    assert (tasks / 'build.db-wal').stat().st_size > 0
    descriptor = os.open(tasks, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        yield tasks, descriptor
    finally:
        os.close(descriptor)
        writer.close()


def test_backup_reads_committed_wal_and_keeps_source_open(wal_database, tmp_path):
    tasks, descriptor = wal_database
    original = {name: (tasks / name).read_bytes() for name in ('build.db', 'build.db-wal', 'build.db-shm')}
    target = tmp_path / 'snapshot.db'
    backup_database_fd(descriptor, target)
    with sqlite3.connect(target) as copied:
        assert copied.execute('SELECT value FROM records').fetchall() == [('committed in WAL',)]
        assert copied.execute('PRAGMA integrity_check').fetchone() == ('ok',)
    assert original == {name: (tasks / name).read_bytes() for name in original}
    assert os.fstat(descriptor).st_ino == tasks.stat().st_ino


def test_ancestor_replacement_cannot_redirect_sqlite(wal_database, tmp_path):
    tasks, descriptor = wal_database
    outside = tmp_path / 'outside'
    outside.mkdir()
    with sqlite3.connect(outside / 'build.db') as other:
        other.execute('CREATE TABLE records (value TEXT)')
        other.execute("INSERT INTO records VALUES ('outside sentinel')")
    before = (outside / 'build.db').read_bytes()
    retained = tmp_path / 'retained'
    tasks.rename(retained)
    tasks.symlink_to(outside, target_is_directory=True)
    try:
        with connect_fd(descriptor) as source:
            assert source.execute('SELECT value FROM records').fetchall() == [('committed in WAL',)]
        assert (outside / 'build.db').read_bytes() == before
    finally:
        tasks.unlink()
        retained.rename(tasks)


@pytest.mark.parametrize('leaf', ['build.db', 'build.db-wal', 'build.db-shm'])
def test_database_and_sidecar_links_refuse_without_touching_destination(wal_database, tmp_path, leaf):
    tasks, descriptor = wal_database
    victim = tasks / leaf
    outside = tmp_path / ('outside-' + leaf)
    outside.write_bytes(victim.read_bytes())
    before = outside.read_bytes()
    retained = tasks / (leaf + '.retained')
    victim.rename(retained)
    victim.symlink_to(outside)
    try:
        with pytest.raises((BuildctlError, sqlite3.Error)):
            with connect_fd(descriptor) as source:
                source.execute('SELECT value FROM records').fetchall()
        assert outside.read_bytes() == before
    finally:
        victim.unlink()
        retained.rename(victim)


def test_sqlite_opens_only_private_copies_and_private_directory_is_0700(wal_database, tmp_path, monkeypatch):
    tasks, descriptor = wal_database
    copied_files = []
    actual_copy = migrate_sqlite.copy_open
    actual_connect = sqlite3.connect

    def record_copy(source, target, **kwargs):
        assert stat.S_IMODE(target.parent.stat().st_mode) == 0o700
        copied_files.append(target)
        return actual_copy(source, target, **kwargs)

    def checked_connect(database, *args, **kwargs):
        assert str(tasks) not in str(database)
        assert '/proc/self/fd/' not in str(database)
        return actual_connect(database, *args, **kwargs)

    monkeypatch.setattr(migrate_sqlite, 'copy_open', record_copy)
    monkeypatch.setattr(sqlite3, 'connect', checked_connect)
    backup_database_fd(descriptor, tmp_path / 'snapshot.db')
    assert {path.name for path in copied_files} == {'build.db', 'build.db-wal', 'build.db-shm'}
    assert all(not path.exists() for path in copied_files)


def test_context_closes_connection_on_exception(wal_database):
    _, descriptor = wal_database
    with pytest.raises(RuntimeError, match='injected'):
        with connect_fd(descriptor) as source:
            raise RuntimeError('injected')
    with pytest.raises(sqlite3.ProgrammingError, match='closed'):
        source.execute('SELECT 1')


def test_connections_have_independent_snapshot_lifetimes(wal_database):
    _, descriptor = wal_database
    with connect_fd(descriptor) as first:
        with connect_fd(descriptor) as second:
            assert second.execute('SELECT value FROM records').fetchone() == ('committed in WAL',)
        assert first.execute('SELECT value FROM records').fetchone() == ('committed in WAL',)


def test_custom_database_name(tmp_path):
    with sqlite3.connect(tmp_path / 'store.db') as original:
        original.execute('CREATE TABLE records (value TEXT)')
        original.execute("INSERT INTO records VALUES ('named')")
    descriptor = os.open(tmp_path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        with connect_fd(descriptor, 'store.db') as source:
            assert source.execute('SELECT value FROM records').fetchone() == ('named',)
    finally:
        os.close(descriptor)


@pytest.mark.parametrize('name', ['', '.', '..', '../outside.db', '/outside.db', 'db?mode=rw', 'db\x00'])
def test_database_name_cannot_escape_directory(wal_database, name):
    _, descriptor = wal_database
    with pytest.raises(BuildctlError, match='basename'):
        with connect_fd(descriptor, name):
            pytest.fail('unsafe filename was accepted')


def test_live_cursor_and_manual_close_are_safe(wal_database):
    _, descriptor = wal_database
    with connect_fd(descriptor) as source:
        cursor = source.execute('SELECT value FROM records')
        source.close()
    with pytest.raises(sqlite3.ProgrammingError, match='closed'):
        cursor.fetchone()


@pytest.mark.parametrize('leaf', ['build.db', 'build.db-wal', 'build.db-shm'])
def test_nonregular_source_leaves_are_refused_without_opening(wal_database, leaf):
    tasks, descriptor = wal_database
    victim = tasks / leaf
    retained = tasks / (leaf + '.retained')
    victim.rename(retained)
    os.mkfifo(victim)
    try:
        with pytest.raises((BuildctlError, sqlite3.Error), match='regular file'):
            with connect_fd(descriptor):
                pytest.fail('FIFO accepted')
    finally:
        victim.unlink()
        retained.rename(victim)


@pytest.mark.parametrize('leaf', ['build.db', 'build.db-wal', 'build.db-shm'])
def test_database_copy_rejects_links_swapped_after_enumeration(wal_database, tmp_path, monkeypatch, leaf):
    tasks, descriptor = wal_database
    victim = tasks / leaf
    outside = tmp_path / ('outside-' + leaf)
    outside.write_bytes(victim.read_bytes())
    before = outside.read_bytes()
    retained = tasks / (leaf + '.retained')
    actual_open = migrate_sqlite.open_entry

    @contextmanager
    def swap_before_open(name, parent_fd):
        if name == leaf:
            victim.rename(retained)
            victim.symlink_to(outside)
        with actual_open(name, parent_fd) as opened:
            yield opened

    monkeypatch.setattr(migrate_sqlite, 'open_entry', swap_before_open)
    try:
        with pytest.raises(BuildctlError, match='safely open|symlink|changed'):
            backup_database_fd(descriptor, tmp_path / 'copy.db')
        assert outside.read_bytes() == before
    finally:
        victim.unlink()
        retained.rename(victim)


def test_connect_path_pins_all_ancestors(wal_database, tmp_path):
    tasks, _ = wal_database
    with connect_path(tasks / 'build.db') as source:
        assert source.execute('SELECT value FROM records').fetchone() == ('committed in WAL',)
    alias = tmp_path / 'alias'
    alias.symlink_to(tasks, target_is_directory=True)
    with pytest.raises((BuildctlError, OSError)):
        with connect_path(alias / 'build.db'):
            pytest.fail('symlink ancestor accepted')


def test_snapshot_connection_is_read_only(wal_database):
    _, descriptor = wal_database
    with connect_fd(descriptor) as source:
        with pytest.raises(sqlite3.Error, match='readonly'):
            source.execute("INSERT INTO records VALUES ('unexpected')")


def test_optional_rollback_journal_is_preserved_in_private_copy(wal_database, monkeypatch):
    tasks, descriptor = wal_database
    (tasks / 'build.db-journal').write_bytes(b'')
    copied_names = set()
    actual_copy = migrate_sqlite.copy_open

    def record_copy(source, target, **kwargs):
        copied_names.add(target.name)
        return actual_copy(source, target, **kwargs)

    monkeypatch.setattr(migrate_sqlite, 'copy_open', record_copy)
    with connect_fd(descriptor) as source:
        assert source.execute('SELECT value FROM records').fetchone() == ('committed in WAL',)
    assert 'build.db-journal' in copied_names


def test_database_hash_verification_detects_copy_corruption(wal_database, tmp_path, monkeypatch):
    _, descriptor = wal_database
    actual_copy = migrate_sqlite.copy_open

    def corrupt_copy(source, target, **kwargs):
        hashes = actual_copy(source, target, **kwargs)
        if target.name == 'build.db':
            target.write_bytes(b'corrupted after copying')
        return hashes

    monkeypatch.setattr(migrate_sqlite, 'copy_open', corrupt_copy)
    with pytest.raises(BuildctlError, match='hash verification'):
        backup_database_fd(descriptor, tmp_path / 'snapshot.db')


def test_hot_rollback_journal_recovers_only_private_copy(tmp_path):
    database = tmp_path / 'build.db'
    subprocess.run([sys.executable, '-c', '''
import os, sqlite3, sys
db = sqlite3.connect(sys.argv[1])
db.execute('PRAGMA journal_mode=DELETE')
db.execute('PRAGMA cache_size=1')
db.execute('CREATE TABLE records (value BLOB)')
db.executemany('INSERT INTO records VALUES (?)', [(b'old' * 4000,)] * 40)
db.commit()
db.execute('BEGIN IMMEDIATE')
db.execute('UPDATE records SET value=?', (b'new' * 4000,))
os._exit(0)
''', str(database)], check=True)
    journal = tmp_path / 'build.db-journal'
    assert journal.stat().st_size > 0
    original = {path: path.read_bytes() for path in (database, journal)}
    with connect_path(database) as source:
        assert source.execute('SELECT DISTINCT value FROM records').fetchall() == [(b'old' * 4000,)]
    assert original == {path: path.read_bytes() for path in original}


@pytest.mark.parametrize('leaf', ['build.db', 'build.db-wal', 'build.db-shm'])
def test_late_fifo_swap_is_rejected_without_blocking(wal_database, tmp_path, monkeypatch, leaf):
    tasks, descriptor = wal_database
    victim, retained = tasks / leaf, tasks / (leaf + '.retained')
    actual_open = migrate_sqlite.open_entry

    @contextmanager
    def swap_before_open(name, parent_fd):
        if name == leaf:
            victim.rename(retained)
            os.mkfifo(victim)
        with actual_open(name, parent_fd) as opened:
            yield opened

    monkeypatch.setattr(migrate_sqlite, 'open_entry', swap_before_open)
    try:
        with pytest.raises(BuildctlError, match='regular file'):
            backup_database_fd(descriptor, tmp_path / 'copy.db')
    finally:
        victim.unlink()
        retained.rename(victim)


def test_live_preview_takes_one_consistent_online_wal_snapshot(wal_database, monkeypatch):
    tasks, _ = wal_database
    original = {name: (tasks / name).read_bytes() for name in ('build.db', 'build.db-wal')}
    actual_connect = sqlite3.connect
    calls = []

    def record_connect(path, *args, **kwargs):
        calls.append(str(path))
        return actual_connect(path, *args, **kwargs)

    monkeypatch.setattr(sqlite3, 'connect', record_connect)
    with preview_input(tasks / 'build.db', live=True) as snapshot:
        assert snapshot != tasks / 'build.db'
        assert stat.S_IMODE(snapshot.parent.stat().st_mode) == 0o700
        with actual_connect(snapshot) as copied:
            assert copied.execute('SELECT value FROM records').fetchone() == ('committed in WAL',)
            assert copied.execute('PRAGMA integrity_check').fetchone() == ('ok',)
    assert not snapshot.exists()
    assert [call for call in calls if str(tasks) in call] == [(tasks / 'build.db').as_uri() + '?mode=ro']
    assert original == {name: (tasks / name).read_bytes() for name in original}


@pytest.mark.parametrize('leaf', ['build.db', 'build.db-wal', 'build.db-shm'])
def test_live_preview_refuses_existing_symlinks_before_sqlite_open(wal_database, tmp_path, monkeypatch, leaf):
    tasks, _ = wal_database
    victim, retained = tasks / leaf, tasks / (leaf + '.retained')
    outside = tmp_path / 'outside'
    outside.write_bytes(victim.read_bytes())
    victim.rename(retained)
    victim.symlink_to(outside)
    monkeypatch.setattr(sqlite3, 'connect', lambda *args, **kwargs: pytest.fail('opened SQLite before symlink refusal'))
    try:
        with pytest.raises(BuildctlError, match='safely open|symlink'):
            with preview_input(tasks / 'build.db', live=True):
                pytest.fail('unsafe live preview accepted')
    finally:
        victim.unlink()
        retained.rename(victim)


def test_nonlive_preview_input_does_not_open_source(tmp_path, monkeypatch):
    missing = tmp_path / 'absent.db'
    monkeypatch.setattr(sqlite3, 'connect', lambda *args, **kwargs: pytest.fail('opened stopped input'))
    with preview_input(missing) as path:
        assert path == missing
