"""The explicit-environment command line boundary."""

import argparse
import json
from pathlib import Path
import sys

from .app import App, default_fetch
from .bridge import Bridge
from .registry import load_registry
from .runtime import BuildctlError, Runner, public_data, redact

MUTATIONS = {"deploy", "rollback", "pair", "prune"}


def parser():
    root = argparse.ArgumentParser(prog="buildctl")
    root.add_argument(
        "--env", required=True, help="explicit environment from environments.toml"
    )
    groups = root.add_subparsers(dest="group", required=True)
    groups.add_parser("status", help="local bridge and hosted app status")
    bridge = groups.add_parser("bridge").add_subparsers(dest="command", required=True)
    bridge.add_parser("deploy").add_argument("ref", nargs="?")
    bridge.add_parser("status")
    bridge.add_parser("rollback")
    bridge.add_parser("pair")
    bridge.add_parser("prune").add_argument("--keep", type=nonnegative, default=3)
    bridge.add_parser("logs").add_argument("-f", "--follow", action="store_true")
    app = groups.add_parser("app").add_subparsers(dest="command", required=True)
    deploy = app.add_parser("deploy")
    deploy.add_argument("ref")
    deploy.add_argument(
        "--force",
        action="store_true",
        help="allow replacement with an exact force-with-lease",
    )
    app.add_parser("status")
    return root


def nonnegative(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be non-negative")
    return number


def registry_file():
    # Editable uv invocation reads the one checked-in file; wheels include it.
    packaged = Path(__file__).with_name("environments.toml")
    return (
        packaged
        if packaged.exists()
        else Path(__file__).parents[1] / "environments.toml"
    )


def bridge_command(args, bridge):
    actions = {
        "deploy": lambda: bridge.deploy(args.ref),
        "status": bridge.status,
        "rollback": bridge.rollback,
        "pair": bridge.pair,
        "prune": lambda: bridge.prune(args.keep),
        "logs": lambda: bridge.logs(args.follow),
    }
    return actions[args.command]()


def combined_status(bridge, app):
    result = {"app": app.status()}
    try:
        result["bridge"] = bridge.status()
    except (BuildctlError, OSError):
        result["bridge_error"] = "could not read local bridge status"
    return result


def dispatch(args, env, runner, repo, fetch):
    if getattr(args, "command", None) in MUTATIONS:
        env.require_mutation()
    if args.group == "app":
        app = App(env, runner, repo, fetch=fetch)
        return (
            app.deploy(args.ref, force=args.force)
            if args.command == "deploy"
            else app.status()
        )
    bridge = Bridge(env, runner, repo)
    if args.group == "status":
        return combined_status(bridge, App(env, runner, repo, fetch=fetch))
    return bridge_command(args, bridge)


def main(
    argv=None,
    *,
    runner=None,
    home=None,
    repo=None,
    registry_path=None,
    fetch=default_fetch,
):
    args = parser().parse_args(argv)
    try:
        envs = load_registry(registry_path or registry_file(), home or Path.home())
        if args.env not in envs:
            raise BuildctlError(
                "unknown environment; add a registry entry and bridge channel support first"
            )
        result = dispatch(
            args,
            envs[args.env],
            runner or Runner(),
            repo or Path(__file__).resolve().parents[3],
            fetch,
        )
        if result is not None:
            print(json.dumps(public_data(result), indent=2))
        return 0
    except (BuildctlError, OSError) as error:
        message = (
            str(error)
            if isinstance(error, BuildctlError)
            else "local operation failed; check paths and access"
        )
        print("buildctl: " + redact(message), file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130
