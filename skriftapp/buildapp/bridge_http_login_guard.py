"""Guard authentication completion until the outer session serializes cookies.

Pinned Skrift places configured user middleware inside its session middleware;
Litestar's inner exception handler sends converted responses through this guard.
The guard owns no database transaction and performs no authentication itself.
"""

import sys

from litestar.types import ASGIApp, Receive, Scope, Send

from buildapp.bridge_http_login import FAILURE_KEY, GUARD_KEY, completion_owner


def clear_failed_login(scope):
    scope[FAILURE_KEY] = True
    scope.get("session", {}).clear()


class BrowserLoginGuard:
    def __init__(self, app: ASGIApp):
        self.app = app

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or completion_owner(scope) is None:
            await self.app(scope, receive, send)
            return
        scope[GUARD_KEY] = True

        async def guarded_send(message):
            if message["type"] == "http.response.start":
                # Python retains the handled exception while Litestar awaits its
                # error response. Preserve this marker even for a custom handler
                # that converts an exception to a redirect or another 2xx/3xx.
                failed = sys.exception() is not None or scope.get(FAILURE_KEY)
                if failed or message["status"] >= 400:
                    clear_failed_login(scope)
            await send(message)

        try:
            await self.app(scope, receive, guarded_send)
        except BaseException:
            # Includes cancellation, before or after the supported login hook.
            # Headers already sent cannot be recalled; no database work is undone.
            clear_failed_login(scope)
            raise
