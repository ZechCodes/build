"""Select public-page icons while assembling a hosted channel's app image.

Sign-in, the public shell, and the generated homepage share /landing/favicon.*.
Their URLs stay stable; the dev image receives the same inverted assets as its SPA.
"""

from pathlib import Path
from shutil import copyfile
import sys


def install_landing_icons(app_config: str, app_dir: Path) -> None:
    if app_config != "app.channel-dev.yaml":
        return
    for extension in ("png", "svg"):
        copyfile(
            app_dir / "static" / f"favicon-dev.{extension}",
            app_dir / "landing" / f"favicon.{extension}",
        )


if __name__ == "__main__":
    install_landing_icons(sys.argv[1], Path(__file__).parent)
