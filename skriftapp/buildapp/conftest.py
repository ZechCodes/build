"""The fixtures every buildapp test may ask for by name. The wiring behind them lives
in ``db_test_support``; this file only hands it to pytest."""

from __future__ import annotations

import pytest
import pytest_asyncio

from buildapp.db_test_support import (
    create_skrift_tables,
    engine_for,
    in_memory_session_maker,
)
from buildapp.email_test_support import RecordingEmailBackend
from buildapp.skrift_app_test_support import build_skrift_app


@pytest_asyncio.fixture()
async def db():
    """One session over a fresh in-memory database with the whole schema in it."""
    session_maker = in_memory_session_maker()
    engine = engine_for(session_maker)
    await create_skrift_tables(engine)
    async with session_maker() as session:
        yield session
    await engine.dispose()


@pytest.fixture()
def email_backend() -> RecordingEmailBackend:
    return RecordingEmailBackend()


@pytest.fixture()
def skrift_app(tmp_path, monkeypatch):
    """The whole app as Skrift builds it, passkeys as the sign-in method, over a
    throwaway database with the schema in it."""
    yield from build_skrift_app(tmp_path, monkeypatch)
