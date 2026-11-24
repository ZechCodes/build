"""The explicit-environment command line boundary."""

import argparse
import json
import os
from pathlib import Path
import sys
from uuid import UUID

from .app import App, default_fetch
from .bridge import Bridge
from .migrate import Migrator
from .service import service_manager
from .registry import load_registry
from .runtime import BuildctlError, Runner, public_data, redact

MUTATIONS = {"deploy", "rollback", "pair", "prune"}


class CommandParser(argparse.ArgumentParser):
    def error(self, message):
        if self.prog.startswith("buildctl secrets") or getattr(self, "secret_context", False):
            self.exit(2, "invalid_arguments\n")
        super().error(message)

    def parse_args(self, args=None, namespace=None):
        parsed = super().parse_args(args, namespace)
        if parsed.group != "migrate" and not parsed.env:
            self.error("--env is required")
        if parsed.group == "migrate":
            if parsed.env:
                self.error("migrate uses explicit --from/--to; omit --env")
            if parsed.rollback and (
                parsed.dry_run or parsed.force or parsed.prod_stopped
            ):
                self.error("--rollback cannot be combined with migration flags")
        if parsed.group == "account" and parsed.account_type is None and parsed.tunnel_access is None:
            self.error("account set requires --type or --tunnel-access")
        return parsed


def parser():
    root = CommandParser(prog="buildctl")
    root.add_argument("--env", help="explicit environment from environments.toml")
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
    account = groups.add_parser("account", help="manage hosted-dev account policy")
    change = account.add_subparsers(dest="command", required=True).add_parser("set")
    change.add_argument("--user-id", required=True, type=UUID)
    change.add_argument("--type", dest="account_type", choices=("free", "admin"))
    change.add_argument("--tunnel-access", choices=("on", "off"))
    change.add_argument("--dry-run", action="store_true")
    change.add_argument("--context", help="verified Kubernetes dev context (or DEV_CONTEXT)")
    secrets = groups.add_parser("secrets").add_subparsers(dest="command", required=True)
    secrets.add_parser("get").add_argument("--id", required=True)
    migration = groups.add_parser("migrate", help="copy prod state into dev")
    migration.add_argument("--from", dest="source", choices=["prod"], required=True)
    migration.add_argument("--to", dest="target", choices=["dev"], required=True)
    migration.add_argument("--dry-run", action="store_true")
    migration.add_argument("--force", action="store_true")
    migration.add_argument("--prod-stopped", action="store_true")
    migration.add_argument("--rollback", type=Path, metavar="BACKUP")
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
    if args.group == "account":
        return account_command(args, env, runner)
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


def kubernetes_actor(runner, kubectl):
    result = runner.run([*kubectl, "auth", "whoami", "-o", "json"])
    try:
        username = json.loads(result.stdout)["status"]["userInfo"]["username"]
    except (ValueError, KeyError, TypeError):
        raise BuildctlError("could not read the Kubernetes operator identity") from None
    if not isinstance(username, str) or not username.strip():
        raise BuildctlError("could not read the Kubernetes operator identity")
    return f"kubernetes:{username}"


def account_command(args, env, runner):
    if env.name != "dev":
        raise BuildctlError("account policy command refused outside dev")
    env.require_mutation()
    context = args.context or os.environ.get("DEV_CONTEXT")
    if not context:
        raise BuildctlError("account set requires an explicit dev Kubernetes context")
    kubectl = ["kubectl", "--context", context, "--namespace", "build-dev"]
    actor = kubernetes_actor(runner, kubectl)
    command = [
        *kubectl, "exec", "deployment/build-app", "--container", "app", "--",
        "python", "-m", "buildapp.account_policy", "set", "--env", env.name,
        "--user-id", str(args.user_id), "--actor", actor,
    ]
    if args.account_type is not None:
        command.extend(["--type", args.account_type])
    if args.tunnel_access is not None:
        command.extend(["--tunnel-access", args.tunnel_access])
    if args.dry_run:
        command.append("--dry-run")
    result = runner.run(command)
    try:
        report = json.loads(result.stdout)
    except ValueError:
        raise BuildctlError("could not read the account policy result") from None
    if not isinstance(report, dict):
        raise BuildctlError("could not read the account policy result")
    return report


def migrate_command(args, envs, runner):
    source, target = envs[args.source], envs[args.target]
    if not args.dry_run:
        target.require_mutation()
    migrator = Migrator(
        source.state_root,
        target.state_root,
        source_prefix=source.state_root,
        projects_dir=source.home / "Projects",
        worktrees=source.home / "Projects/.build-worktrees",
        states=lambda: {
            name: service_manager(envs[name], runner).state()
            for name in ("prod", "dev")
        },
    )
    if args.rollback:
        return migrator.rollback(args.rollback)
    return migrator.run(
        dry_run=args.dry_run, force=args.force, prod_stopped=args.prod_stopped
    )


def environment_command(args, envs, runner, repo, fetch):
    if args.env not in envs:
        raise BuildctlError(
            "unknown environment; add a registry entry and bridge channel support first"
        )
    return dispatch(
        args,
        envs[args.env],
        runner or Runner(),
        repo or Path(__file__).resolve().parents[3],
        fetch,
    )


def secrets_command(args):
    binary = os.environ.get("BRIDGE_SECRET_BRIDGE", "")
    if not Path(binary).is_absolute():
        print("unauthorized", file=sys.stderr)
        return 1
    try:
        os.execv(binary, [binary, "secrets", "get", "--env", args.env, "--id", args.id])
        return 0
    except OSError:
        print("secret_unavailable", file=sys.stderr)
        return 1


def main(
    argv=None,
    *,
    runner=None,
    home=None,
    repo=None,
    registry_path=None,
    fetch=default_fetch,
):
    command_parser = parser()
    raw_args = list(sys.argv[1:] if argv is None else argv)
    command_parser.secret_context = "secrets" in raw_args[:3]
    args = command_parser.parse_args(raw_args)
    if args.group == "secrets":
        return secrets_command(args)
    try:
        envs = load_registry(registry_path or registry_file(), home or Path.home())
        if args.group == "migrate":
            result = migrate_command(args, envs, runner or Runner())
        else:
            result = environment_command(args, envs, runner, repo, fetch)
        if result is not None:
            mutation = getattr(args, "command", None) in MUTATIONS
            print(json.dumps(public_data(result), indent=None if mutation else 2))
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
