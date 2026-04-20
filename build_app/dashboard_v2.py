"""Dashboard v2 controller — parallel route for the rewrite.

v2 is being built alongside v1 at /dashboard-v2/. Once v2 reaches parity
it will be promoted to /dashboard/ and v1 deleted. See
planning/dashboard-v2/07-migration.md.
"""

from __future__ import annotations

from litestar import Controller, Request, get
from litestar.response import Redirect, Template

from skrift.auth.session_keys import SESSION_USER_ID


class DashboardV2Controller(Controller):
    """Dashboard v2 — parallel rewrite target."""

    path = "/dashboard-v2"

    @get("/", exclude_from_auth=True)
    async def dashboard_v2(self, request: Request) -> Template | Redirect:
        user_id = request.session.get(SESSION_USER_ID)
        if not user_id:
            return Redirect(path="/auth/login?next=/dashboard-v2/")

        user_name = request.session.get("user_name", "Admin")
        return Template(
            "dashboard_v2.html",
            context={
                "user_name": user_name,
            },
        )
