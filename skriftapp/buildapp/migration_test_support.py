"""Reading a migration without running it, in one place.

A migration is a frozen snapshot: it is loaded by path (it is not importable — the
filename carries a timestamp and the module is never on ``sys.path``) and its
``upgrade``/``downgrade`` are exercised with ``alembic.op`` patched out, so a test can
assert what a revision *would* create without a database. Every ``test_*_migration``
file reads its revision through here."""

from __future__ import annotations

import importlib.util
from pathlib import Path
from types import ModuleType
from unittest.mock import patch

MIGRATIONS_DIR = Path(__file__).resolve().parent.parent / "migrations" / "versions"


def migration_path(filename: str) -> Path:
    return MIGRATIONS_DIR / filename


def load_migration(path: Path) -> ModuleType:
    """The revision at ``path`` as a module, loaded by location rather than import."""
    spec = importlib.util.spec_from_file_location(path.stem, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def upgrade_calls(path: Path):
    """``(create_table call args, create_index call list)`` for one ``upgrade()``."""
    migration = load_migration(path)
    with patch.object(migration.op, "create_table") as create_table, patch.object(
        migration.op, "create_index"
    ) as create_index, patch.object(migration.op, "f", side_effect=lambda name: name):
        migration.upgrade()
    create_table.assert_called_once()
    return create_table.call_args, create_index.call_args_list


def downgrade_calls(path: Path):
    """``(drop_table mock, drop_index mock)`` for one ``downgrade()``."""
    migration = load_migration(path)
    with patch.object(migration.op, "drop_table") as drop_table, patch.object(
        migration.op, "drop_index"
    ) as drop_index, patch.object(migration.op, "f", side_effect=lambda name: name):
        migration.downgrade()
    return drop_table, drop_index


def table_elements(path: Path) -> tuple[str, list]:
    """The table name and the ``sa`` elements one ``create_table`` was given."""
    (table_name, *elements), _ = upgrade_calls(path)[0]
    return table_name, elements
