"""The database and ASGI wiring the tests share, so no test file copies another's
fixture.

One in-memory database (StaticPool, so every session in a test reaches the same
connection), one schema builder, one account factory, one Litestar builder for the
routes that need a session cookie, and one jinja environment for the admin templates.
The pytest fixtures over these live in ``conftest.py``."""

from __future__ import annotations

from collections.abc import AsyncIterator
from pathlib import Path
from uuid import UUID, uuid4

from jinja2 import ChoiceLoader, DictLoader, Environment, FileSystemLoader
from litestar import Litestar
from litestar.di import Provide
from litestar.middleware.session.client_side import CookieBackendConfig
from skrift.auth.services import invalidate_user_permissions_cache
from skrift.db.base import Base
from skrift.db.models.role import Role, RolePermission
from skrift.db.models.user import User
from sqlalchemy.ext.asyncio import (
    AsyncEngine,
    AsyncSession,
    async_sessionmaker,
    create_async_engine,
)
from sqlalchemy.pool import StaticPool

IN_MEMORY_DATABASE_URL = "sqlite+aiosqlite:///:memory:"
SESSION_SECRET = b"0123456789abcdef0123456789abcdef"
ADMINISTRATOR = "administrator"
ADMIN_BASE_TEMPLATE = "admin/base.html"
ADMIN_BASE_STUB = "{% block admin_content %}{% endblock %}"
TEMPLATES_DIR = Path(__file__).resolve().parent.parent / "templates"


def in_memory_session_maker() -> async_sessionmaker[AsyncSession]:
    """A session maker over a fresh in-memory database."""
    engine = create_async_engine(IN_MEMORY_DATABASE_URL, poolclass=StaticPool)
    return async_sessionmaker(engine, expire_on_commit=False)


def engine_for(session_maker: async_sessionmaker[AsyncSession]) -> AsyncEngine:
    """The engine a maker was built over, so a caller carries one value rather than
    two and still gets to create its schema and dispose it."""
    return session_maker.kw["bind"]


async def create_skrift_tables(engine: AsyncEngine) -> None:
    """Every framework table plus buildapp's, so the real auth guard can resolve a
    user's roles the way it does in production."""
    async with engine.begin() as connection:
        await connection.run_sync(Base.metadata.create_all)


async def add_account(session: AsyncSession, email: str, *, administrator: bool = False) -> UUID:
    user = User(id=uuid4(), email=email, name="Someone")
    if administrator:
        role = Role(id=uuid4(), name=ADMINISTRATOR)
        role.permissions.append(RolePermission(id=uuid4(), permission=ADMINISTRATOR))
        user.roles.append(role)
    session.add(user)
    await session.commit()
    invalidate_user_permissions_cache(user.id)
    return user.id


def session_backend_config() -> CookieBackendConfig:
    return CookieBackendConfig(secret=SESSION_SECRET)


def session_app(
    route_handlers: list,
    *,
    session_maker: async_sessionmaker[AsyncSession],
    session_config: CookieBackendConfig,
) -> Litestar:
    """The stack a session-carrying route needs: a ``db_session`` dependency over
    ``session_maker``, the cookie session the user id and the CSRF token live in, and
    both names the app reads its maker back through — ``make_session`` for a test that
    wants to look at the rows, ``session_maker_class`` for the auth guard."""
    engine = engine_for(session_maker)

    async def provide_db_session() -> AsyncIterator[AsyncSession]:
        async with session_maker() as session:
            yield session

    async def create_tables(app: Litestar) -> None:
        await create_skrift_tables(engine)

    async def dispose_engine(app: Litestar) -> None:
        await engine.dispose()

    app = Litestar(
        route_handlers=route_handlers,
        dependencies={"db_session": Provide(provide_db_session)},
        middleware=[session_config.middleware],
        on_startup=[create_tables],
        on_shutdown=[dispose_engine],
    )
    app.state.make_session = session_maker
    app.state.session_maker_class = session_maker
    return app


def admin_template_environment() -> Environment:
    """The real admin templates with ``admin/base.html`` stubbed, so a template test
    renders the page under test and nothing of the framework's chrome."""
    return Environment(  # nosemgrep: python.flask.security.xss.audit.direct-use-of-jinja2.direct-use-of-jinja2
        loader=ChoiceLoader(
            [
                DictLoader({ADMIN_BASE_TEMPLATE: ADMIN_BASE_STUB}),
                FileSystemLoader(str(TEMPLATES_DIR)),
            ]
        ),
        autoescape=True,
    )
