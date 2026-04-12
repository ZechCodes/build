"""Theme version endpoint — returns the active theme's version from theme.yaml."""

from litestar import get
from litestar.response import Response

from skrift.db.services.setting_service import get_cached_site_theme
from skrift.lib.theme import get_theme_info


@get("/api/theme/version")
async def theme_version() -> Response:
    theme_name = get_cached_site_theme()
    info = get_theme_info(theme_name) if theme_name else None
    return Response(content={"version": info.version if info else ""})
