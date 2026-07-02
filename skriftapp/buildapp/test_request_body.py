"""Request-body shape guard: a syntactically valid but non-object JSON body
(``[]``, ``"x"``, ``123``) must be a 400 ``ClientException``, never an unhandled
``TypeError``/``AttributeError`` → 500."""

from __future__ import annotations

import pytest
from litestar.exceptions import ClientException

from buildapp.request_body import require_json_object


def test_non_object_bodies_raise_client_exception():
    for bad_body in ([], "x", 123, 1.5, True, None):
        with pytest.raises(ClientException):
            require_json_object(bad_body)


def test_object_bodies_pass_through():
    body = {"code": "G6ZP-KD2U"}
    assert require_json_object(body) is body
