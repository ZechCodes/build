"""Online SQLite snapshots, structured rewrites and complete row verification."""

import hashlib
import json
from pathlib import Path
import sqlite3

from .migrate_paths import JSON_COLUMNS, references
from .runtime import BuildctlError


def identifier(name):
    return '"' + name.replace('"', '""') + '"'


def connect(path):
    return sqlite3.connect(path.as_uri() + '?mode=ro', uri=True)


def backup_database(source, target):
    with connect(source) as original, sqlite3.connect(target) as copied:
        original.backup(copied)


def table_names(db):
    return [row[0] for row in db.execute(
        "SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]


def rows(db, table):
    # All current bridge tables are rowid tables. Explicitly refuse unsupported
    # future schemas rather than guessing an update key or silently losing rows.
    cursor = db.execute(f'SELECT rowid, * FROM {identifier(table)} ORDER BY rowid')
    columns = [col[0] for col in cursor.description][1:]
    for row in cursor:
        yield columns, row[0], row[1:]


def scan_database(path, paths):
    tables, rewrites, refs, fingerprints = {}, {}, set(), {}
    historical = set()
    with connect(path) as db:
        for table in table_names(db):
            count, changes, found, signature, terminal = scan_table(db, table, paths)
            tables[table] = count
            if changes:
                rewrites[table] = changes
            refs.update(found)
            historical.update(terminal)
            fingerprints[table] = signature
    return {'tables': tables, 'rewrites': rewrites, 'references': refs,
            'fingerprints': fingerprints, 'terminal_operation_roots': historical}


def scan_table(db, table, paths):
    count, changes, found, historical = 0, {}, set(), set()
    signature = hashlib.sha256()
    for columns, rowid, row in rows(db, table):
        mapped = tuple(paths.cell(value, col) for col, value in zip(columns, row))
        signature.update(repr((rowid, mapped)).encode())
        count += 1
        status = dict(zip(columns, row)).get('status')
        for col, old, new in zip(columns, row, mapped):
            if old != new:
                changes[col] = changes.get(col, 0) + 1
            required, terminal = classified_references(table, col, status, old)
            found.update(required)
            historical.update(terminal)
    return count, changes, found, signature.hexdigest(), historical


def classified_references(table, column, status, value):
    refs = set(cell_references(value, column))
    if table != 'operations' or column != 'delivery' or status not in {'claimed', 'delivered'}:
        return refs, set()
    terminal = {value for key, value in refs if key == 'root'}
    return {(key, value) for key, value in refs if key != 'root'}, terminal


def cell_references(value, column):
    if not isinstance(value, str):
        return []
    if column in JSON_COLUMNS and value.lstrip().startswith(('{', '[')):
        try:
            return list(references(json.loads(value)))
        except ValueError:
            return []
    return list(references({column: value}))


def rewrite_database(path, paths):
    with sqlite3.connect(path) as db:
        for table in table_names(db):
            # Separate read cursor and updates; rowid order remains unchanged.
            for columns, rowid, row in rows(db, table):
                updates = {col: paths.cell(value, col) for col, value in zip(columns, row)
                           if paths.cell(value, col) != value}
                if updates:
                    assignments = ', '.join(identifier(col) + '=?' for col in updates)
                    db.execute(f'UPDATE {identifier(table)} SET {assignments} WHERE rowid=?',
                               (*updates.values(), rowid))
        if db.execute('PRAGMA integrity_check').fetchone()[0] != 'ok':
            raise BuildctlError('migrated SQLite integrity check failed')


def verify_database(path, expected):
    class Unchanged:
        @staticmethod
        def cell(value, column):
            return value
    actual = scan_database(Path(path), Unchanged())
    if actual['tables'] != expected['tables'] or actual['fingerprints'] != expected['fingerprints']:
        raise BuildctlError('SQLite row or record verification failed')
