"""Validate an offline dev kustomize render, pin images and split rollout phases.

No cluster calls. The workflow applies only these checked namespaced documents;
Namespace/RBAC/Secret bootstrap is deliberately refused here.
"""
import argparse
import re
from copy import deepcopy
from pathlib import Path

import yaml

APP_REPOSITORY = "ghcr.io/zechcodes/build-app-dev"
RELAY_REPOSITORY = "ghcr.io/zechcodes/build-relay-dev"
RESOURCES = {
    ("Service", "build-postgres"): "infrastructure",
    ("StatefulSet", "build-postgres"): "infrastructure",
    ("Job", "build-app-migrate"): "migrate",
    ("Deployment", "build-app"): "app",
    ("Service", "build-app"): "app",
    ("Ingress", "build-app"): "app",
    ("Deployment", "build-relay"): "relay",
    ("Service", "build-relay"): "relay",
    ("Ingress", "build-relay"): "relay",
}


def pinned_image(image, repository):
    if not re.fullmatch(re.escape(repository) + r"@sha256:[0-9a-f]{64}", image):
        raise ValueError(f"expected a pinned digest in {repository}")
    return image


def pin_pod(document, app_image, relay_image):
    kind = document["kind"]
    if kind not in ("Deployment", "Job", "StatefulSet"):
        return
    containers = document["spec"]["template"]["spec"]["containers"]
    if len(containers) != 1:
        raise ValueError("unexpected dev pod containers")
    container = containers[0]
    if kind == "StatefulSet":
        if container["image"] != "docker.io/library/postgres:16":
            raise ValueError("unexpected dev database image")
        return
    repository = RELAY_REPOSITORY if document["metadata"]["name"] == "build-relay" else APP_REPOSITORY
    if container["image"] != repository + ":unbuilt":
        raise ValueError("unexpected source image; use the dev image placeholder")
    replacement = relay_image if repository == RELAY_REPOSITORY else app_image
    if replacement is not None:
        container["image"] = replacement


def selected_phases(target):
    return {"all": {"infrastructure", "migrate", "app", "relay"},
            "app": {"infrastructure", "migrate", "app"}, "relay": {"relay"}}[target]


def render(documents, app_image, relay_image, *, target="all"):
    selected = selected_phases(target)
    if "app" in selected:
        app_image = pinned_image(app_image or "", APP_REPOSITORY)
    if "relay" in selected:
        relay_image = pinned_image(relay_image or "", RELAY_REPOSITORY)
    phases = {name: [] for name in ("infrastructure", "migrate", "app", "relay")}
    seen = set()
    for document in deepcopy(documents):
        metadata = document["metadata"]
        key = document["kind"], metadata["name"]
        if metadata.get("namespace") != "build-dev" or key not in RESOURCES or key in seen:
            raise ValueError("unexpected resource or namespace in dev render")
        seen.add(key)
        pin_pod(document, app_image, relay_image)
        phases[RESOURCES[key]].append(document)
    if seen != RESOURCES.keys():
        raise ValueError("incomplete dev resource set")
    return {name: docs for name, docs in phases.items() if name in selected}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--target", choices=("all", "app", "relay"), default="all")
    parser.add_argument("--app-image")
    parser.add_argument("--relay-image")
    args = parser.parse_args()
    documents = list(yaml.safe_load_all(args.input.read_text()))
    phases = render(documents, args.app_image, args.relay_image, target=args.target)
    args.output.mkdir(parents=True, exist_ok=True)
    for name, resources in phases.items():
        (args.output / f"{name}.yaml").write_text(yaml.safe_dump_all(resources, sort_keys=False))


if __name__ == "__main__":
    main()
