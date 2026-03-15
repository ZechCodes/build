"""Dashboard controller for Build — admin-only fleet overview."""

from __future__ import annotations

from litestar import Controller, Request, get
from litestar.response import Template

from skrift.auth.guards import auth_guard, Permission


class DashboardController(Controller):
    """Agent fleet dashboard, locked to admin users."""

    path = "/dashboard"
    guards = [auth_guard, Permission("administrator")]

    @get("/")
    async def dashboard(self, request: Request) -> Template:
        """Render the fleet dashboard."""
        user_name = request.session.get("user_name", "Admin")
        return Template(
            "dashboard.html",
            context={
                "user_name": user_name,
            },
        )
