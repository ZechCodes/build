"""Build web application entry point.

Uses Skrift CMS as the ASGI framework. Controllers, page types, and
middleware are configured in app.yaml (or app.development.yaml when
SKRIFT_ENV=development).
"""

from skrift.asgi import app  # noqa: F401

if __name__ == "__main__":
    import subprocess
    import sys

    sys.exit(subprocess.call([
        sys.executable, "-m", "hypercorn", "main:app",
        "--bind", "0.0.0.0:8000", "--reload",
    ]))
