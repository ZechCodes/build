"""Tests pinning the production config (app.yaml) against the deployment contract:
passkey-only auth, rate limiting on, env-driven Postgres URL, same-origin CSP with
only the relay websocket as an external connect target — and that dev conveniences
(dummy auth, localhost internal fallback, esm.sh/Google-fonts CSP) stay in
app.dev.yaml only."""

from __future__ import annotations

from pathlib import Path

import yaml

_SKRIFTAPP_DIR = Path(__file__).resolve().parent.parent


def _load(config_name: str) -> dict:
    return yaml.safe_load((_SKRIFTAPP_DIR / config_name).read_text())


def _csp_directives(config: dict) -> dict[str, list[str]]:
    csp = config["security_headers"]["content_security_policy"]
    directives: dict[str, list[str]] = {}
    for clause in csp.split(";"):
        parts = clause.strip().split()
        if parts:
            directives[parts[0]] = parts[1:]
    return directives


# ----- auth -------------------------------------------------------------------


def test_production_auth_is_passkey_only():
    methods = _load("app.yaml")["auth"]["methods"]
    method_types = {m["type"] for m in methods.values()}
    assert method_types == {"passkey"}


def test_dummy_auth_stays_dev_only():
    dev_types = {m["type"] for m in _load("app.dev.yaml")["auth"]["methods"].values()}
    assert "dummy" in dev_types


def test_production_redirect_base_url_pins_passkey_origin():
    assert _load("app.yaml")["auth"]["redirect_base_url"] == "https://getbuild.ing"


# ----- hardening --------------------------------------------------------------


def test_production_rate_limiting_enabled():
    assert _load("app.yaml")["rate_limit"]["enabled"] is True


def test_production_database_url_is_env_driven():
    assert _load("app.yaml")["db"]["url"] == "$DATABASE_URL"


def test_production_disallows_internal_localhost_fallback():
    config = _load("app.yaml")
    assert config.get("internal_api", {}).get("allow_localhost", False) is False


# ----- CSP --------------------------------------------------------------------


def test_production_csp_has_no_third_party_origins():
    directives = _csp_directives(_load("app.yaml"))
    allowed_external = {"wss://relay.getbuild.ing"}
    for directive, sources in directives.items():
        for source in sources:
            # nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
            if source.startswith(("http://", "https://", "ws://", "wss://")):
                assert source in allowed_external, f"{directive} allows {source}"
            assert "esm.sh" not in source
            assert "googleapis" not in source and "gstatic" not in source


def test_production_csp_connect_src_is_self_plus_relay():
    directives = _csp_directives(_load("app.yaml"))
    assert set(directives["connect-src"]) == {"'self'", "wss://relay.getbuild.ing"}


def test_production_csp_core_directives_are_self():
    directives = _csp_directives(_load("app.yaml"))
    assert directives["default-src"] == ["'self'"]
    assert "'self'" in directives["script-src"]
    assert "'unsafe-inline'" not in directives["script-src"]
    assert "'unsafe-eval'" not in directives["script-src"]
    assert directives["form-action"] == ["'self'"]
    assert directives["base-uri"] == ["'self'"]
    assert directives["frame-ancestors"] == ["'none'"]
    assert directives["object-src"] == ["'none'"]


def test_production_csp_nonce_enabled():
    assert _load("app.yaml")["security_headers"]["csp_nonce"] is True
