"""Request-body shape guard shared by every JSON-consuming route: a syntactically
valid but non-object body (``[]``, ``"x"``, ``123``) must be a 400
``ClientException``, never an unhandled ``TypeError``/``AttributeError`` → 500. So must
a body that is not JSON at all."""

from __future__ import annotations

from litestar import Request
from litestar.exceptions import ClientException, SerializationException

MALFORMED_JSON_MESSAGE = "request body must be valid JSON"
NON_OBJECT_MESSAGE = "request body must be a JSON object"


def require_json_object(body: object) -> dict:
    """Return ``body`` when it is a JSON object; raise ``ClientException`` otherwise."""
    if not isinstance(body, dict):
        raise ClientException(NON_OBJECT_MESSAGE)
    return body


async def read_json_object(request: Request) -> dict:
    """Parse the request body as a JSON object, 400ing on either failure."""
    try:
        parsed = await request.json()
    except SerializationException as malformed:
        raise ClientException(MALFORMED_JSON_MESSAGE) from malformed
    return require_json_object(parsed)
