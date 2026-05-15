"""Dashboard controller for Build — admin-only fleet overview."""

from __future__ import annotations

from litestar import Controller, Request, get
from litestar.response import Redirect, Template

from skrift.auth.session_keys import SESSION_USER_ID


class DashboardController(Controller):
    """Agent fleet dashboard. Redirects to login if not authenticated."""

    path = "/dashboard"

    @get("/", exclude_from_auth=True)
    async def dashboard(self, request: Request) -> Template | Redirect:
        """Render the fleet dashboard, or redirect to login."""
        user_id = request.session.get(SESSION_USER_ID)
        if not user_id:
            return Redirect(path="/auth/login?next=/dashboard/")

        user_name = request.session.get("user_name", "Admin")
        return Template(
            "dashboard.html",
            context={
                "user_name": user_name,
            },
        )


class DashboardV2Controller(Controller):
    """Parallel dashboard v2 preview. Redirects to login if not authenticated."""

    path = "/dashboard-v2"

    @get("/", exclude_from_auth=True)
    async def dashboard_v2(self, request: Request) -> Template | Redirect:
        """Render the dashboard v2 preview, or redirect to login."""
        user_id = request.session.get(SESSION_USER_ID)
        if not user_id:
            return Redirect(path="/auth/login?next=/dashboard-v2/")

        user_name = request.session.get("user_name", "Admin")
        return Template(
            "dashboard-v2.html",
            context={
                "user_name": user_name,
            },
        )
