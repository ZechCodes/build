"""Build app controller — serves the plan/diff SPA behind Skrift auth."""

from pathlib import Path
from uuid import UUID

from litestar import Controller, Request, get
from litestar.response import Redirect, Response
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.db.models.user import User

HERE = Path(__file__).parent


class BuildController(Controller):
    path = "/app"

    @get("/", sync_to_thread=False)
    async def index(self, request: Request, db_session: AsyncSession) -> Response | Redirect:
        # Skrift auth: a logged-in user has user_id in the encrypted session.
        user_id = request.session.get("user_id")
        if not user_id:
            return Redirect("/auth/login")

        result = await db_session.execute(select(User).where(User.id == UUID(user_id)))
        user = result.scalar_one_or_none()
        email = getattr(user, "email", None) or "user"

        html = (HERE / "build.html").read_text().replace("{{USER}}", email)
        return Response(html, media_type="text/html")
