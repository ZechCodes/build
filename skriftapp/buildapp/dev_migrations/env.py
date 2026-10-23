"""Dev-only migration environment: never constructs a connection or resolves a URL.

The operator runner injects one verified connection. Historical unqualified DDL
uses its dev-only search_path; the version table is explicitly schema-qualified.
"""
from alembic import context
from skrift.db.base import Base

connection = context.config.attributes['connection']
context.configure(
    connection=connection,
    target_metadata=Base.metadata,
    version_table_schema=context.config.attributes['schema'],
)
with context.begin_transaction():
    context.run_migrations()
