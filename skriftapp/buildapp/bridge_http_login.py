"""Establish browser HTTP authority identity at authentication, never lazily.

Skrift calls after_login after final authentication/session rotation and before
returning the first authenticated response, for primary and deferred logins.
Every later signed cookie version therefore carries the same persisted identity.
"""
import secrets

from litestar.exceptions import NotAuthorizedException
from skrift.db.models.user import User
from skrift.hooks import action
from sqlalchemy import select

from buildapp.bridge_http_models import BridgeHttpClient
from buildapp.desktop_auth import BRIDGE_ACCOUNT_KEY, BRIDGE_BINDING_KEY, auth_session_maker
from buildapp.session_auth import session_user_id
from buildapp.token_hash import token_hash


@action("after_login")
async def establish_browser_login(login_result, request):
    """Persist before exposing identity; failed persistence clears authentication."""
    try:
        user_id = session_user_id(request)
        if user_id is None or user_id != login_result.user.id:
            raise NotAuthorizedException("Authentication required")
        binding = "browser:" + secrets.token_urlsafe(32)
        async with auth_session_maker(request)() as db:
            user = await db.scalar(select(User).where(User.id == user_id).with_for_update())
            if user is None:
                raise NotAuthorizedException("Authentication required")
            db.add(BridgeHttpClient(user_id=user_id, binding_hash=token_hash(binding)))
            await db.commit()
        request.session[BRIDGE_ACCOUNT_KEY] = str(user_id)
        request.session[BRIDGE_BINDING_KEY] = binding
    except BaseException:
        # Includes cancelled persistence: no failed login response may carry an
        # authenticated cookie whose durable identity was never established.
        request.session.clear()
        raise
