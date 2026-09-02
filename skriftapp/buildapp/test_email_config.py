"""Tests pinning the outbound-email deployment contract: the production SMTP block in
app.yaml, the credential-free console block in app.dev.yaml, the live-send app.mail.yaml,
and the four secret keys the k8s manifest, the bootstrap script and the README must all
name together."""

from __future__ import annotations

import re

import yaml

from buildapp.test_production_config import (
    PRODUCTION_BASE_URL,
    SKRIFTAPP_DIR,
    load_config,
)
from buildapp.waitlist_mail import NOTIFY_ADDRESS_ENV

INTERPOLATED_ENV_VARS = ("SMTP_USERNAME", "SMTP_PASSWORD", "SMTP_FROM_ADDRESS")
EMAIL_ENV_VARS = (*INTERPOLATED_ENV_VARS, NOTIFY_ADDRESS_ENV)
SMTP_BACKEND = "skrift.lib.email_backends:SMTPEmailBackend"
CONSOLE_BACKEND = "skrift.lib.email_backends:ConsoleEmailBackend"
SMTP_HOST_VALUE = "smtp.fastmail.com"
SMTP_PORT_VALUE = 587
LOCAL_PUBLIC_BASE_URL = "http://localhost:8090"

REPO_ROOT = SKRIFTAPP_DIR.parent
K8S_APP_MANIFEST_PATH = REPO_ROOT / "deploy" / "k8s" / "app.yaml"
BOOTSTRAP_SCRIPT_PATH = REPO_ROOT / "deploy" / "k8s" / "bootstrap-secrets.sh"
README_PATH = SKRIFTAPP_DIR / "README.md"
BOOTSTRAP_MARKER = "# --- build-app smtp (add-if-missing)"


def _env_tokens(block: dict) -> set[str]:
    return {
        match.group(1)
        for value in block.values()
        for match in re.finditer(r"\$([A-Z_]+)", str(value))
    }


def _bootstrap_smtp_block() -> str:
    lines = BOOTSTRAP_SCRIPT_PATH.read_text().splitlines()
    start = next(index for index, line in enumerate(lines) if line.startswith(BOOTSTRAP_MARKER))
    remaining = lines[start + 1 :]
    end = next(
        (index for index, line in enumerate(remaining) if line.startswith("# ---")),
        len(remaining),
    )
    return "\n".join(remaining[:end])


def _deployment_container_env() -> list[dict]:
    documents = list(yaml.safe_load_all(K8S_APP_MANIFEST_PATH.read_text()))
    deployment = next(doc for doc in documents if doc and doc["kind"] == "Deployment")
    container = deployment["spec"]["template"]["spec"]["containers"][0]
    return container["env"]


def test_production_email_block_uses_fastmail_smtp():
    email = load_config("app.yaml")["email"]
    assert email["backend"] == SMTP_BACKEND
    assert email["smtp_host"] == SMTP_HOST_VALUE
    assert email["smtp_port"] == SMTP_PORT_VALUE
    assert email["smtp_starttls"] is True
    assert email["public_base_url"] == PRODUCTION_BASE_URL


def test_production_reply_to_equals_from_address():
    email = load_config("app.yaml")["email"]
    assert email["reply_to"] == email["from_address"]


def test_production_email_block_interpolates_exactly_the_three_smtp_vars():
    assert _env_tokens(load_config("app.yaml")["email"]) == set(INTERPOLATED_ENV_VARS)


def test_production_yaml_does_not_interpolate_the_notify_address():
    assert f"${NOTIFY_ADDRESS_ENV}" not in (SKRIFTAPP_DIR / "app.yaml").read_text()


def test_dev_email_block_uses_console_backend_without_credentials():
    assert load_config("app.dev.yaml")["email"]["backend"] == CONSOLE_BACKEND
    assert "$" not in (SKRIFTAPP_DIR / "app.dev.yaml").read_text()


def test_dev_public_base_url_is_local():
    assert load_config("app.dev.yaml")["email"]["public_base_url"] == LOCAL_PUBLIC_BASE_URL


def test_mail_config_is_dev_plus_the_production_email_block():
    mail = load_config("app.mail.yaml")
    dev = load_config("app.dev.yaml")
    production_email = load_config("app.yaml")["email"]
    assert {key: value for key, value in mail.items() if key != "email"} == {
        key: value for key, value in dev.items() if key != "email"
    }
    assert mail["email"] == {
        **production_email,
        "public_base_url": LOCAL_PUBLIC_BASE_URL,
    }


def test_mail_config_registers_the_waitlist_controller():
    controllers = load_config("app.mail.yaml")["controllers"]
    assert "buildapp.waitlist_controller:WaitlistController" in controllers


def test_k8s_deployment_requires_every_interpolated_email_secret_key():
    env_entries = {entry["name"]: entry for entry in _deployment_container_env()}
    for name in INTERPOLATED_ENV_VARS:
        assert name in env_entries, f"{name} is not passed to the app container"
        secret_key_ref = env_entries[name]["valueFrom"]["secretKeyRef"]
        assert secret_key_ref["name"] == "build-app"
        assert secret_key_ref["key"] == name
        assert "optional" not in secret_key_ref, (
            f"{name} must be required: app.yaml interpolation fails without it"
        )


def test_k8s_deployment_treats_the_notify_address_as_optional():
    env_entries = {entry["name"]: entry for entry in _deployment_container_env()}
    secret_key_ref = env_entries[NOTIFY_ADDRESS_ENV]["valueFrom"]["secretKeyRef"]
    assert secret_key_ref["name"] == "build-app"
    assert secret_key_ref["key"] == NOTIFY_ADDRESS_ENV
    assert secret_key_ref["optional"] is True


def test_bootstrap_script_adds_smtp_keys_only_from_the_operator_environment():
    block = _bootstrap_smtp_block()
    for name in EMAIL_ENV_VARS:
        assert f"require_env {name}" in block
    assert "random_secret(" not in block
    assert "openssl" not in block
    assignment = re.compile(r"^\s*(" + "|".join(EMAIL_ENV_VARS) + ")=")
    for line in BOOTSTRAP_SCRIPT_PATH.read_text().splitlines():
        assert assignment.match(line) is None, f"bootstrap invents a credential: {line}"


def test_bootstrap_script_repairs_a_secret_missing_any_one_of_the_four_keys():
    block = _bootstrap_smtp_block()
    guard = block[: block.index("then")]
    for name in EMAIL_ENV_VARS:
        assert name in guard, f"a secret missing only {name} is never repaired"


def test_bootstrap_script_never_splices_a_credential_into_the_patch_json():
    block = _bootstrap_smtp_block()
    assert "stringData" not in block
    for name in EMAIL_ENV_VARS:
        assert f"${{{name}}}" not in block
        assert f'b64_value "${name}"' in block


def test_bootstrap_script_defines_require_env_that_exits_nonzero():
    script = BOOTSTRAP_SCRIPT_PATH.read_text()
    assert "require_env()" in script
    assert "exit 1" in script


def test_readme_documents_every_email_variable_and_the_mail_env():
    readme = README_PATH.read_text()
    for name in EMAIL_ENV_VARS:
        assert name in readme
    assert "SKRIFT_ENV=mail" in readme
    assert SMTP_HOST_VALUE in readme
