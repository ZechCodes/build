"""The whole app as Skrift builds it, for the tests that must go through Skrift's own
session config, middleware stack, template engine and auth controller rather than a
Litestar app assembled by hand: ``app.dev.yaml`` with production's sign-in method and
CSP, over a throwaway SQLite file. The ``skrift_app`` fixture over this lives in
``conftest.py``."""

from __future__ import annotations

import re
from collections.abc import Awaitable, Callable, Iterator
from pathlib import Path
from typing import TypeVar

from litestar.testing import TestClient
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine

from buildapp.db_test_support import create_skrift_tables

SKRIFTAPP_DIR = Path(__file__).resolve().parents[1]
TEMPLATES_DIR = SKRIFTAPP_DIR / "templates"
DATABASE_FILE = "app.db"
#: The dev app with production's sign-in method in place of the dummy login.
DEV_CONFIG_EDITS = {
    "    dummy:\n      type: dummy\n      label: Demo Login": "    passkey:\n      type: passkey\n      label: Passkey",
    "  csp_nonce: false": "  csp_nonce: true",
}
PRODUCTION_CONFIG = (SKRIFTAPP_DIR / "app.yaml").read_text()
#: Skrift marks the session cookie Secure outside debug, so the client must be on https
#: for the cookie to come back.
SECURE_ORIGIN = "https://testserver.local"

T = TypeVar("T")


def build_skrift_app(tmp_path: Path, monkeypatch) -> Iterator:
    """Yield the app ``create_app`` builds from a copy of app.dev.yaml in ``tmp_path``,
    its schema already in place, and clear Skrift's cached settings either side."""
    import asyncio

    from skrift.asgi import create_app
    from skrift.config import get_settings

    dev_config = (SKRIFTAPP_DIR / "app.dev.yaml").read_text()
    for stock, ours in DEV_CONFIG_EDITS.items():
        assert stock in dev_config, stock
        dev_config = dev_config.replace(stock, ours)
    # Production's CSP, nonce and all, so the page is held to what getbuild.ing sends.
    production_csp = re.search(r"  content_security_policy: .*", PRODUCTION_CONFIG).group(0)
    dev_config = re.sub(r"  content_security_policy: .*", lambda _: production_csp, dev_config)
    database = tmp_path / DATABASE_FILE
    (tmp_path / "app.dev.yaml").write_text(dev_config.replace(f"./{DATABASE_FILE}", str(database)))
    (tmp_path / "templates").symlink_to(TEMPLATES_DIR)
    monkeypatch.setenv("SKRIFT_ENV", "dev")
    monkeypatch.setenv("SECRET_KEY", "a-test-secret-that-is-long-enough-to-use")
    monkeypatch.chdir(tmp_path)
    asyncio.run(_create_schema(database))
    get_settings.cache_clear()
    try:
        yield create_app()
    finally:
        get_settings.cache_clear()


async def _create_schema(database: Path) -> None:
    engine = create_async_engine(_database_url(database))
    await create_skrift_tables(engine)
    await engine.dispose()


def _database_url(database: Path) -> str:
    return f"sqlite+aiosqlite:///{database}"


def on_database(client: TestClient, work: Callable[[AsyncSession], Awaitable[T]]) -> T:
    """Run ``work`` over a session on the app's database file, on the client's loop.
    The app was built in the working directory ``build_skrift_app`` moved to, which a
    test is still in."""

    async def run() -> T:
        engine = create_async_engine(_database_url(Path(DATABASE_FILE).resolve()))
        try:
            async with async_sessionmaker(engine, expire_on_commit=False)() as session:
                return await work(session)
        finally:
            await engine.dispose()

    with client.portal() as portal:
        return portal.call(run)
