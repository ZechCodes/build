"""Request-body shape guard shared by every JSON-consuming route: a syntactically
valid but non-object body (``[]``, ``"x"``, ``123``) must be a 400
``ClientException``, never an unhandled ``TypeError``/``AttributeError`` → 500."""

from __future__ import annotations

from litestar.exceptions import ClientException


def require_json_object(body: object) -> dict:
    """Return ``body`` when it is a JSON object; raise ``ClientException`` otherwise."""
    if not isinstance(body, dict):
        raise ClientException("request body must be a JSON object")
    return body
