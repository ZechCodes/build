"""Dev deploy isolation contracts; no cluster access or workflow dispatch."""
from copy import deepcopy
import importlib.util
from pathlib import Path
import runpy
import subprocess

import pytest
import yaml

ROOT = Path(__file__).resolve().parents[2]
DEV = ROOT / "deploy/k8s/dev"
APP_IMAGE = "ghcr.io/zechcodes/build-app-dev@sha256:" + "a" * 64
RELAY_IMAGE = "ghcr.io/zechcodes/build-relay-dev@sha256:" + "b" * 64


def config(name):
    return yaml.safe_load((ROOT / "skriftapp" / name).read_text())


def test_channel_config_is_production_with_dev_endpoints_and_database_isolation():
    prod = config("app.yaml")
    dev = config("app.channel-dev.yaml")
    assert dev["auth"]["redirect_base_url"] == "https://dev.getbuild.ing"
    assert dev["email"]["public_base_url"] == "https://dev.getbuild.ing"
    assert dev["internal_api"]["allow_localhost"] is False
    assert dev["auth"]["methods"] == {"passkey": {"type": "passkey", "label": "Passkey"}}
    policy = dev["security_headers"]["content_security_policy"]
    assert "connect-src 'self' data: wss://relay-dev.getbuild.ing;" in policy
    assert "wss://relay.getbuild.ing" not in policy
    expected = deepcopy(prod)
    expected["auth"]["redirect_base_url"] = "https://dev.getbuild.ing"
    expected["email"]["public_base_url"] = "https://dev.getbuild.ing"
    expected["security_headers"]["content_security_policy"] = prod["security_headers"]["content_security_policy"].replace(
        "wss://relay.getbuild.ing", "wss://relay-dev.getbuild.ing"
    )
    expected["db"].update(pool_size=2, pool_overflow=0)
    assert dev == expected
    assert prod["auth"]["redirect_base_url"] == "https://getbuild.ing"


def test_image_defaults_preserve_prod_and_accept_explicit_dev_inputs():
    source = (ROOT / "skriftapp/Containerfile").read_text()
    assert "ARG APP_CONFIG=app.yaml" in source
    assert "COPY skriftapp/${APP_CONFIG} ./app.yaml" in source
    assert "ARG VITE_BUILD_CHANNEL=prod" in source
    assert 'VITE_BUILD_CHANNEL="$VITE_BUILD_CHANNEL"' in source
    assert "ARG VITE_RELAY_URL=wss://relay.getbuild.ing" in source
    assert "ARG VITE_BUILD_VERSION=dev" in source


def dev_resources():
    kustomization = yaml.safe_load((DEV / "kustomization.yaml").read_text())
    assert kustomization["namespace"] == "build-dev"
    docs = []
    for name in kustomization["resources"]:
        assert "/" not in name and name.endswith(".yaml")
        docs.extend(yaml.safe_load_all((DEV / name).read_text()))
    return docs


def test_manifests_are_namespace_local_with_no_cluster_bootstrap():
    docs = dev_resources()
    assert {d["kind"] for d in docs} == {"Deployment", "Service", "Ingress", "Job"}
    assert all(d["metadata"]["namespace"] == "build-dev" for d in docs)
    assert yaml.safe_load((DEV / "namespace.yaml").read_text())["metadata"]["name"] == "build-dev"
    serialized = yaml.safe_dump_all(docs)
    for forbidden in ("8ly", "getbuild.ing-tls", "build-app:latest", "build-relay:latest"):
        assert forbidden not in serialized
    by_name = {(d["kind"], d["metadata"]["name"]): d for d in docs}
    app = by_name["Deployment", "build-app"]
    relay = by_name["Deployment", "build-relay"]
    assert relay["spec"]["strategy"]["type"] == "Recreate"
    env = {e["name"]: e for e in relay["spec"]["template"]["spec"]["containers"][0]["env"]}
    for key in ("API_INTERNAL_URL", "RELAY_API_URL"):
        assert env[key]["value"] == "http://build-app.build-dev.svc.cluster.local:8080"
    assert env["RELAY_INTERNAL_SECRET"]["valueFrom"]["secretKeyRef"]["name"] == "build-relay"
    app_env = {e["name"]: e for e in app["spec"]["template"]["spec"]["containers"][0]["env"]}
    assert app_env["DATABASE_URL"]["valueFrom"]["secretKeyRef"]["name"] == "build-app"
    assert not (DEV / "postgres.yaml").exists()
    assert all(d["metadata"]["name"] != "build-postgres" for d in docs)
    for name, host in (("build-app", "dev.getbuild.ing"), ("build-relay", "relay-dev.getbuild.ing")):
        ingress = by_name["Ingress", name]
        assert ingress["spec"]["rules"][0]["host"] == host
        assert ingress["spec"]["tls"][0]["hosts"] == [host]
        assert ingress["metadata"]["annotations"]["traefik.ingress.kubernetes.io/router.entrypoints"] == "websecure"


def test_operator_ci_rbac_has_only_required_namespace_permissions():
    """Pin direct API grants, not indirect secret reads or Ingress host claims."""
    resources = list(yaml.safe_load_all((DEV / "ci-rbac.yaml").read_text()))
    assert len(resources) == 3
    by_kind = {document["kind"]: document for document in resources}
    assert set(by_kind) == {"ServiceAccount", "Role", "RoleBinding"}
    for document in resources:
        assert document["metadata"] == {"namespace": "build-dev", "name": "dev-deploy"}
        assert document["apiVersion"] == ("v1" if document["kind"] == "ServiceAccount"
                                          else "rbac.authorization.k8s.io/v1")
    account = by_kind["ServiceAccount"]
    assert set(account) == {"apiVersion", "kind", "metadata", "automountServiceAccountToken"}
    assert account["automountServiceAccountToken"] is False
    role = by_kind["Role"]
    assert set(role) == {"apiVersion", "kind", "metadata", "rules"}
    actual = set()
    for rule in role["rules"]:
        assert set(rule) == {"apiGroups", "resources", "verbs"}
        actual.update((group, resource, verb) for group in rule["apiGroups"]
                      for resource in rule["resources"] for verb in rule["verbs"])
    read = {"get", "list", "watch"}
    write = read | {"create", "update", "patch"}
    expected = {
        (group, resource, verb)
        for group, resource, verbs in [
            ("", "services", write), ("apps", "deployments", write),
            ("apps", "replicasets", read), ("batch", "jobs", write | {"delete"}),
            ("networking.k8s.io", "ingresses", write), ("", "pods", read),
            ("", "pods/log", {"get"}),
        ] for verb in verbs
    }
    assert actual == expected
    binding = by_kind["RoleBinding"]
    assert set(binding) == {"apiVersion", "kind", "metadata", "roleRef", "subjects"}
    assert binding["roleRef"] == {"apiGroup": "rbac.authorization.k8s.io",
                                  "kind": "Role", "name": "dev-deploy"}
    assert binding["subjects"] == [{"kind": "ServiceAccount", "name": "dev-deploy",
                                    "namespace": "build-dev"}]
    kustomization = yaml.safe_load((DEV / "kustomization.yaml").read_text())
    assert "ci-rbac.yaml" not in kustomization["resources"]
    for document in resources:
        with pytest.raises(ValueError, match="unexpected resource or namespace"):
            renderer().render(dev_resources() + [document], APP_IMAGE, RELAY_IMAGE)


def workflow():
    return yaml.safe_load((ROOT / ".github/workflows/dev-deploy.yml").read_text())


def test_spa_checks_do_not_inherit_build_environment():
    flow = workflow()
    checks = flow["jobs"]["checks"]
    spa = next(step for step in checks["steps"] if step.get("name") == "SPA")
    for scope in (flow, checks, spa):
        assert not any(key.startswith("VITE_BUILD_") or key == "VITE_RELAY_URL"
                       for key in scope.get("env", {})), "SPA tests must use the baseline environment"
    assert spa["working-directory"] == "spa"
    assert spa["run"].splitlines() == ["nice -n 10 npm run lint", "nice -n 10 npm test"]

    build = next(step for step in checks["steps"] if step.get("name") == "Build dev SPA")
    assert checks["steps"].index(spa) < checks["steps"].index(build)
    assert build["working-directory"] == "spa"
    assert build["run"] == "nice -n 10 npm run build"
    assert build["env"] == {
        "VITE_BUILD_CHANNEL": "dev",
        "VITE_RELAY_URL": "wss://relay-dev.getbuild.ing",
        "VITE_BUILD_VERSION": "${{ needs.resolve.outputs.sha }}",
    }


def test_automatic_and_manual_workflow_pin_dev_sha_and_only_dev_images():
    flow = workflow()
    triggers = flow.get("on", flow.get(True))
    assert set(triggers) == {"workflow_dispatch", "workflow_run"}
    assert triggers["workflow_run"] == {
        "workflows": ["Dev deploy request"], "types": ["completed"], "branches": ["dev"],
    }
    assert flow["permissions"] == {"contents": "read"}
    jobs = flow["jobs"]
    resolver = jobs["resolve"]
    assert resolver["steps"][0]["with"]["ref"] == "refs/heads/dev"
    assert "git rev-parse HEAD" in resolver["steps"][1]["run"]
    assert "sha" in resolver["outputs"]
    for name, job in jobs.items():
        if name != "resolve":
            checkout = next(s for s in job["steps"] if "actions/checkout@" in s.get("uses", ""))
            assert checkout["with"]["ref"] == "${{ needs.resolve.outputs.sha }}"
        if not name.startswith("deploy-"):
            assert "environment" not in job
            assert "KUBECONFIG" not in yaml.safe_dump(job)
    for name, image in (("build-app", "build-app-dev"), ("build-relay", "build-relay-dev")):
        job = jobs[name]
        assert job["needs"] == ["resolve", "checks"]
        assert job["if"] == "${{ needs.checks.result == 'success' }}"
        build = next(s for s in job["steps"] if "docker/build-push-action@" in s.get("uses", ""))
        assert build["with"]["tags"] == f"ghcr.io/zechcodes/{image}:${{{{ needs.resolve.outputs.sha }}}}"
        assert "digest" in job["outputs"]
    args = next(s for s in jobs["build-app"]["steps"] if "docker/build-push-action@" in s.get("uses", ""))["with"]["build-args"]
    for arg in ("APP_CONFIG=app.channel-dev.yaml", "VITE_BUILD_CHANNEL=dev", "VITE_RELAY_URL=wss://relay-dev.getbuild.ing", "VITE_BUILD_VERSION=${{ needs.resolve.outputs.sha }}"):
        assert arg in args


@pytest.mark.parametrize("name", ["deploy-app", "deploy-relay"])
def test_deploy_requires_success_protection_digest_and_dev_only_apply(name):
    job = workflow()["jobs"][name]
    assert job["environment"] == "development"
    assert job["concurrency"] == {"group": name + "-dev", "cancel-in-progress": False}
    image_job = name.replace("deploy", "build")
    assert job["needs"] == ["resolve", "checks", image_job]
    assert job["if"] == f"${{{{ needs.checks.result == 'success' && needs['{image_job}'].result == 'success' }}}}"
    source = yaml.safe_dump(job)
    assert "DEV_KUBECONFIG" in source
    assert f"needs['{image_job}'].outputs.digest" in source
    assert "--namespace=build-dev" in source
    assert "8ly" not in source and "doctl" not in source
    assert "deploy/k8s/dev" in source
    commands = "\n".join(s.get("run", "") for s in job["steps"])
    assert "infrastructure.yaml" not in commands
    assert "statefulset" not in commands.lower()
    assert "kubectl exec" not in commands
    assert "create secret" not in commands
    assert "create namespace" not in commands
    applied = [line.split("apply -f ", 1)[1] for line in commands.splitlines() if "apply -f " in line]
    assert applied == (["/tmp/dev-deploy/migrate.yaml", "/tmp/dev-deploy/app.yaml"]
                       if name == "deploy-app" else ["/tmp/dev-deploy/relay.yaml"])
    if name == "deploy-app":
        assert commands.index("apply -f /tmp/dev-deploy/migrate.yaml") < commands.index("apply -f /tmp/dev-deploy/app.yaml")
        assert "wait --for=condition=complete job/build-app-migrate" in commands


def test_new_push_cancels_checks_and_builds_but_never_migration_or_rollout():
    flow = workflow()
    # Workflow-level cancellation overrides job-level protection, so forbid it.
    assert "concurrency" not in flow
    for name in ("checks", "build-app", "build-relay"):
        assert flow["jobs"][name]["concurrency"] == {
            "group": "dev-deploy-" + name, "cancel-in-progress": True,
        }
    for name in ("deploy-app", "deploy-relay"):
        assert flow["jobs"][name]["concurrency"] == {
            "group": name + "-dev", "cancel-in-progress": False,
        }
    migration = next(step for step in flow["jobs"]["deploy-app"]["steps"]
                     if step.get("name") == "Migrate dev database before app rollout")
    assert "wait --for=condition=complete" in migration["run"]


def test_signal_is_only_a_notification_and_never_supplies_deployment_data():
    flow = workflow()
    assert flow["jobs"]["resolve"]["steps"][0]["with"]["ref"] == "refs/heads/dev"
    for job in flow["jobs"].values():
        # Only the entry condition may inspect the event's origin/status.
        body = yaml.safe_dump({key: value for key, value in job.items() if key != "if"})
        for forbidden in ("github.event", "inputs.", "download-artifact", "actions/cache", "workflow_run"):
            assert forbidden not in body
        for step in job["steps"]:
            if "actions/checkout@" in step.get("uses", ""):
                assert step["with"]["persist-credentials"] is False


def test_dev_push_signal_has_no_checkout_secrets_or_write_token():
    signal = yaml.safe_load((ROOT / ".github/workflows/dev-deploy-request.yml").read_text())
    assert signal["name"] == "Dev deploy request"
    assert signal.get("on", signal.get(True)) == {"push": {"branches": ["dev"]}}
    assert signal["permissions"] == {}
    assert set(signal["jobs"]) == {"request"}
    job = signal["jobs"]["request"]
    assert "environment" not in job
    assert job["steps"] == [{"run": "echo 'Dev branch moved; main will resolve and deploy its latest commit.'"}]


@pytest.mark.parametrize("event,ref,conclusion,branch,repo,source_event,allowed", [
    ("workflow_run", "main", "success", "dev", "owner/repo", "push", True),
    ("workflow_dispatch", "main", "success", "dev", "owner/repo", "push", True),
    ("workflow_dispatch", "dev", "success", "dev", "owner/repo", "push", False),
    ("push", "main", "success", "dev", "owner/repo", "push", False),
    ("workflow_run", "main", "failure", "dev", "owner/repo", "push", False),
    ("workflow_run", "main", "cancelled", "dev", "owner/repo", "push", False),
    ("workflow_run", "main", "skipped", "dev", "owner/repo", "push", False),
    ("workflow_run", "main", "success", "main", "owner/repo", "push", False),
    ("workflow_run", "main", "success", "dev", "fork/repo", "push", False),
    ("workflow_run", "main", "success", "dev", "owner/repo", "pull_request", False),
    ("workflow_run", "dev", "success", "dev", "owner/repo", "push", False),
])
def test_deploy_entry_accepts_only_main_dispatch_or_same_repository_dev_push(
    event, ref, conclusion, branch, repo, source_event, allowed,
):
    condition = workflow()["jobs"]["resolve"]["if"][3:-2].strip()
    evaluate = runpy.run_path(str(ROOT / ".github/test-transport-workflow.py"))["evaluate_condition"]
    context = {"github": {
        "event_name": event, "ref": f"refs/heads/{ref}", "repository": "owner/repo",
        "event": {"workflow_run": {"conclusion": conclusion, "head_branch": branch,
                                    "head_repository": {"full_name": repo}, "event": source_event}},
    }}
    assert evaluate(condition, context) == allowed


@pytest.mark.parametrize("name", ["deploy-app", "deploy-relay"])
@pytest.mark.parametrize("tip,api_exit,current", [
    ("a" * 40, "0", True), ("b" * 40, "0", False),
    ("invalid", "0", None), ("", "1", None),
])
def test_rollout_rechecks_dev_inside_lock_and_skips_superseded_source(
    tmp_path, name, tip, api_exit, current,
):
    job = workflow()["jobs"][name]
    assert job["concurrency"]["cancel-in-progress"] is False
    guard = job["steps"][0]
    assert guard["id"] == "freshness"
    assert guard["env"] == {
        "GH_TOKEN": "${{ secrets.GITHUB_TOKEN }}",
        "GH_REPO": "${{ github.repository }}",
        "SOURCE_SHA": "${{ needs.resolve.outputs.sha }}",
    }
    assert 'gh api "repos/$GH_REPO/git/ref/heads/dev"' in guard["run"]
    for step in job["steps"][1:]:
        expected = "steps.freshness.outputs.current == 'true'"
        if step.get("name") == "Remove temporary kube credential":
            expected = "always() && " + expected
        assert step["if"] == "${{ " + expected + " }}"
    # Execute the actual inline guard, replacing only the GitHub API transport.
    api = tmp_path / "gh"
    api.write_text('#!/bin/sh\nprintf "%s\\n" "$TEST_DEV_TIP"\nexit "$TEST_GH_EXIT"\n')
    api.chmod(0o700)
    output, summary = tmp_path / "output", tmp_path / "summary"
    result = subprocess.run(
        ["/bin/bash", "-eo", "pipefail", "-c", guard["run"]],
        env={"PATH": f"{tmp_path}:/usr/bin:/bin", "SOURCE_SHA": "a" * 40,
             "GH_REPO": "owner/repo", "TEST_DEV_TIP": tip, "TEST_GH_EXIT": api_exit,
             "GITHUB_OUTPUT": str(output), "GITHUB_STEP_SUMMARY": str(summary)},
        capture_output=True, text=True, check=False,
    )
    if current is None:
        assert result.returncode != 0
        assert not output.exists()
    else:
        assert result.returncode == 0, result.stderr
        assert output.read_text().strip() == f"current={str(current).lower()}"
        if not current:
            assert summary.read_text().strip() == f"superseded by {tip}"


def renderer():
    spec = importlib.util.spec_from_file_location("dev_render", ROOT / "deploy/render_dev.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_renderer_pins_every_image_and_splits_migration_before_rollout():
    phases = renderer().render(dev_resources(), APP_IMAGE, RELAY_IMAGE)
    assert set(phases) == {"migrate", "app", "relay"}
    assert [d["kind"] for d in phases["migrate"]] == ["Job"]
    for phase in ("app", "migrate", "relay"):
        pods = [d for d in phases[phase] if d["kind"] in ("Deployment", "Job")]
        assert len(pods) == 1
        container = pods[0]["spec"]["template"]["spec"]["containers"][0]
        assert container["image"] == (RELAY_IMAGE if phase == "relay" else APP_IMAGE)


@pytest.mark.parametrize("image", ["ghcr.io/zechcodes/build-app:latest", "ghcr.io/zechcodes/build-app-dev:abc", APP_IMAGE[:-1]])
def test_renderer_refuses_non_dev_or_unpinned_images(image):
    with pytest.raises(ValueError):
        renderer().render(dev_resources(), image, RELAY_IMAGE)


@pytest.mark.parametrize("mutation", ["namespace", "cluster-resource", "secret", "unknown-image"])
def test_renderer_refuses_out_of_scope_resources(mutation):
    docs = dev_resources()
    if mutation == "namespace":
        docs[0]["metadata"]["namespace"] = "8ly"
    elif mutation == "cluster-resource":
        docs.append({"kind": "Namespace", "metadata": {"name": "build-dev"}})
    elif mutation == "secret":
        docs.append({"kind": "Secret", "metadata": {"namespace": "build-dev", "name": "unexpected"}})
    else:
        deployment = next(d for d in docs if d["kind"] == "Deployment")
        deployment["spec"]["template"]["spec"]["containers"][0]["image"] = "ghcr.io/zechcodes/build-app:latest"
    with pytest.raises(ValueError):
        renderer().render(docs, APP_IMAGE, RELAY_IMAGE)


@pytest.mark.parametrize("resources", [
    [{"kind": "StatefulSet", "metadata": {"namespace": "build-dev", "name": "build-postgres"}}],
    [{"kind": "Service", "metadata": {"namespace": "build-dev", "name": "build-postgres"}}],
    [{"kind": "PersistentVolumeClaim", "metadata": {"namespace": "build-dev", "name": "data-build-postgres-0"}}],
])
def test_renderer_refuses_dev_database_resources(resources):
    docs = [d for d in dev_resources() if d["metadata"]["name"] != "build-postgres"]
    with pytest.raises(ValueError, match="unexpected resource or namespace"):
        renderer().render(docs + resources, APP_IMAGE, RELAY_IMAGE)


def test_failed_cancelled_or_skipped_gates_never_publish_or_deploy():
    # Reuse its interpreter to test actual failure conditions rather than summaries.
    contract = runpy.run_path(str(ROOT / ".github/test-transport-workflow.py"))
    evaluate = contract["evaluate_condition"]
    for name in ("build-app", "build-relay", "deploy-app", "deploy-relay"):
        condition = workflow()["jobs"][name]["if"][3:-2].strip()
        for result in ("failure", "cancelled", "skipped", "success"):
            context = {"needs": {"checks": {"result": result}, "build-app": {"result": result}, "build-relay": {"result": result}}}
            assert evaluate(condition, context) == (result == "success")


@pytest.mark.parametrize("target", ["app", "relay"])
def test_independent_rollout_requires_only_its_own_digest(target):
    phases = renderer().render(dev_resources(), APP_IMAGE if target == "app" else None,
                               RELAY_IMAGE if target == "relay" else None, target=target)
    assert set(phases) == ({"migrate", "app"} if target == "app" else {"relay"})
    with pytest.raises(ValueError):
        renderer().render(dev_resources(), None, None, target=target)


@pytest.mark.parametrize("config_name,host,relay", [
    ("app.yaml", "getbuild.ing", "relay.getbuild.ing"),
    ("app.channel-dev.yaml", "dev.getbuild.ing", "relay-dev.getbuild.ing"),
])
def test_actual_hosted_cookie_headers_csrf_and_passkey_rp(tmp_path, monkeypatch, config_name, host, relay):
    import asyncio
    from types import SimpleNamespace

    from litestar.testing import TestClient
    from skrift.asgi import create_app
    from skrift.auth.second_factors.passkey_service import _resolve_origin, _resolve_rp_id
    from skrift.config import get_settings

    from buildapp.db_test_support import create_skrift_schema_in

    database = tmp_path / "hosted.db"
    asyncio.run(create_skrift_schema_in(database))
    hosted_config = config(config_name)
    (tmp_path / "app.yaml").write_text(yaml.safe_dump(hosted_config))
    (tmp_path / "templates").symlink_to(ROOT / "skriftapp/templates")
    monkeypatch.chdir(tmp_path)
    for key, value in {"SKRIFT_ENV": "production", "SECRET_KEY": "disposable-unique-" + host,
                       "DATABASE_URL": f"sqlite+aiosqlite:///{database}",
                       "SMTP_USERNAME": "test@example.com", "SMTP_PASSWORD": "disposable",
                       "SMTP_FROM_ADDRESS": "test@example.com"}.items():
        monkeypatch.setenv(key, value)
    get_settings.cache_clear()
    try:
        settings = get_settings()
        request = SimpleNamespace(base_url="https://spoofed.invalid", app=SimpleNamespace(state=SimpleNamespace()))
        assert _resolve_origin(request, settings) == f"https://{host}"
        assert _resolve_rp_id(request, settings) == host
        with TestClient(create_app(), base_url=f"https://{host}") as client:
            page = client.get("/auth/login")
            assert page.status_code == 200
            cookie = page.headers["set-cookie"].lower()
            assert "secure" in cookie and "httponly" in cookie and "samesite=lax" in cookie
            assert "domain=" not in cookie
            assert f"wss://{relay}" in page.headers["content-security-policy"]
            assert "access-control-allow-origin" not in page.headers
            # Exercise onboarding through the real config and dependency wiring.
            downloads = client.get("/app/downloads").json()
            if config_name == "app.channel-dev.yaml":
                assert downloads["install_command"] is None
                assert "source-built" in downloads["installation_notice"]
                assert client.get("/install.sh").status_code == 404
                assert client.get("/install-desktop.sh").status_code == 404
                assert client.get("/app/downloads/linux-x86_64").status_code == 404
                docs = client.get("/docs")
                assert docs.status_code == 200
                assert "source-built" in docs.text
                assert "curl" not in docs.text
            else:
                assert downloads["install_command"] == 'curl -fsSL "https://getbuild.ing/install.sh" | sh'
                assert client.get("/install.sh").status_code == 200
            refused = client.post("/auth/passkey/options", data={"email": "test@example.com"})
            assert refused.status_code == 400
            assert refused.json()["error"] == "invalid_csrf"
    finally:
        get_settings.cache_clear()
