"""Establish browser HTTP authority identity at authentication, never lazily.

Skrift calls after_login after final authentication/session rotation and before
returning the first authenticated response, for primary and deferred logins.
Every later signed cookie version therefore carries the same persisted identity.
"""

import secrets
from functools import lru_cache
from importlib.metadata import PackageNotFoundError, version

from advanced_alchemy.extensions.litestar import SQLAlchemyPlugin
from advanced_alchemy.extensions.litestar._utils import get_aa_scope_state
from litestar.exceptions import NotAuthorizedException
from skrift.db.models.user import User
from skrift.hooks import action
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_object_session

from buildapp.bridge_http_models import BridgeHttpClient
from buildapp.desktop_auth import (
    BRIDGE_ACCOUNT_KEY,
    BRIDGE_BINDING_KEY,
    auth_session_maker,
)
from buildapp.session_auth import session_user_id
from buildapp.token_hash import token_hash

GUARD_KEY = "build_bridge_login_guard"
FAILURE_KEY = "build_bridge_login_failed"


@lru_cache(maxsize=1)
def completion_runtime_matches():
    """Commit and middleware ordering must be re-audited after dependency changes."""
    expected = {
        "skrift": "0.2.0a10",
        "litestar": "2.24.0",
        "advanced-alchemy": "1.11.0",
    }
    try:
        return all(version(package) == pin for package, pin in expected.items())
    except PackageNotFoundError:
        return False


def completion_owner(scope):
    """Pinned resolved callables, never a URL, form field or attachment heuristic.

    #923 audits Skrift 0.2.0a10's actual commit positions. The Build registration
    wrapper delegates to that handler, then performs its existing invite work.
    New/replaced completion callables require another ownership classification.
    """
    from skrift.controllers.auth import AuthController
    from buildapp.auth_controller import BuildAuthController

    if not completion_runtime_matches():
        return None
    handler = scope.get("route_handler")
    fn = getattr(handler, "fn", None)
    fn = getattr(fn, "__func__", fn)
    borrowed = (
        BuildAuthController.complete_primary_method_registration.fn,
        AuthController.complete_primary_method_auth.fn,
    )
    owned = (
        AuthController.oauth_callback.fn,
        AuthController.complete_second_factor_verification.fn,
        AuthController.dummy_login_submit.fn,
        AuthController.verify_email_claim.fn,
    )
    if any(fn is candidate for candidate in borrowed):
        return "borrowed"
    if any(fn is candidate for candidate in owned):
        return "owned"
    return None


def borrowed_session(request, user):
    """Require the same object attachment and configured request unit of work."""
    configs = request.app.plugins.get(SQLAlchemyPlugin).config
    matching = [
        config for config in configs if config.session_dependency_key == "db_session"
    ]
    if len(matching) != 1:
        raise NotAuthorizedException("Authentication completion unavailable")
    db = get_aa_scope_state(request.scope, matching[0].session_scope_key)
    if not isinstance(db, AsyncSession) or async_object_session(user) is not db:
        raise NotAuthorizedException("Authentication completion unavailable")
    return db


async def persist_identity(request, user, binding, owner):
    identity = BridgeHttpClient(user_id=user.id, binding_hash=token_hash(binding))
    if owner == "borrowed":
        db = borrowed_session(request, user)
        db.add(identity)
        await db.flush()
        return
    async with auth_session_maker(request)() as db:
        persisted = await db.scalar(
            select(User).where(User.id == user.id).with_for_update()
        )
        if persisted is None:
            raise NotAuthorizedException("Authentication required")
        db.add(identity)
        await db.commit()


@action("after_login")
async def establish_browser_login(login_result, request):
    """Join the classified transaction; the inner-session guard covers later work."""
    try:
        owner = completion_owner(request.scope)
        if owner is None or request.scope.get(GUARD_KEY) is not True:
            raise NotAuthorizedException("Authentication completion unavailable")
        user_id = session_user_id(request)
        if user_id is None or user_id != login_result.user.id:
            raise NotAuthorizedException("Authentication required")
        binding = "browser:" + secrets.token_urlsafe(32)
        await persist_identity(request, login_result.user, binding, owner)
        request.session[BRIDGE_ACCOUNT_KEY] = str(user_id)
        request.session[BRIDGE_BINDING_KEY] = binding
    except BaseException:
        # Includes cancelled persistence: no failed login response may carry an
        # authenticated cookie whose durable identity was never established.
        request.scope[FAILURE_KEY] = True
        request.session.clear()
        raise
