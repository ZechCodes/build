"""Request-body shape guard: a body that is not JSON at all and a syntactically valid
but non-object body (``[]``, ``"x"``, ``123``) are both a 400 ``ClientException``,
never an unhandled ``TypeError``/``AttributeError``/``SerializationException`` → 500.

``read_json_object`` is the one entry point a JSON route reads its body through, so
every such route answers the same two 400s."""

from __future__ import annotations

import inspect

import pytest
from litestar.exceptions import ClientException, SerializationException

from buildapp import devices_controller, invites_controller, push_controller
from buildapp.request_body import (
    MALFORMED_JSON_MESSAGE,
    NON_OBJECT_MESSAGE,
    read_json_object,
    require_json_object,
)


class _Body:
    """Just enough of a Request for the reader: something with ``.json()``."""

    def __init__(self, parsed=None, *, malformed: bool = False):
        self._parsed = parsed
        self._malformed = malformed

    async def json(self):
        if self._malformed:
            raise SerializationException("not json")
        return self._parsed


def test_non_object_bodies_raise_client_exception():
    for bad_body in ([], "x", 123, 1.5, True, None):
        with pytest.raises(ClientException):
            require_json_object(bad_body)


def test_object_bodies_pass_through():
    body = {"code": "G6ZP-KD2U"}
    assert require_json_object(body) is body


@pytest.mark.asyncio
async def test_a_body_that_is_not_json_at_all_is_a_client_error():
    with pytest.raises(ClientException) as refusal:
        await read_json_object(_Body(malformed=True))
    assert refusal.value.detail == MALFORMED_JSON_MESSAGE
    assert refusal.value.status_code == 400


@pytest.mark.asyncio
async def test_json_that_is_not_an_object_is_a_client_error():
    with pytest.raises(ClientException) as refusal:
        await read_json_object(_Body(["invitee@example.com"]))
    assert refusal.value.detail == NON_OBJECT_MESSAGE


@pytest.mark.asyncio
async def test_an_object_body_is_returned_as_it_was_parsed():
    body = {"email": "invitee@example.com"}
    assert await read_json_object(_Body(body)) is body


def test_no_json_route_reads_a_body_any_other_way():
    """One spelling, so no route is left answering 500 for a truncated body."""
    for module in (invites_controller, devices_controller, push_controller):
        source = inspect.getsource(module)
        assert "request.json()" not in source, module.__name__
