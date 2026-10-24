"""Offline, staged prod-to-dev state cutover. Never manages service lifecycles."""

from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import shutil
import sqlite3
import tempfile
import uuid

from .migrate_lifecycle import classify
from .migrate_git import apply_remotes, plan_remotes
from .migrate_paths import Paths, references, scratch_directory
from .migrate_store import backup_database, rewrite_database, scan_database, verify_database
from .runtime import BuildctlError, Runner, digest

IMPORTED = ('config.json', 'projects', 'provider-recovery', 'review-receivers', 'harness', 'tasks')
OPERATIONAL = {'identity.json', 'log', 'builds', 'buildctl.json', 'buildctl-candidate', 'buildctl.lock'}
WORKTREE_KEYS = {'worktree_path', 'checkout', 'checkout_path', 'working_repository', 'root', 'cwd'}


def no_symlinks(path, *, tree=False):
    if path.resolve() != path:
        raise BuildctlError('migration path uses a symlink or noncanonical component')
    if tree and path.is_dir():
        for current, dirs, files in os.walk(path):
            if any((Path(current) / name).is_symlink() for name in (*dirs, *files)):
                raise BuildctlError('migration tree contains a symlink')


def remove(path):
    if path.is_dir():
        shutil.rmtree(path)
    elif path.exists():
        path.unlink()


def copy_entry(source, target):
    if source.is_dir():
        shutil.copytree(source, target)
    else:
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)


@contextmanager
def migration_lock(target):
    target.parent.mkdir(parents=True, exist_ok=True)
    lock = target.parent / ('.' + target.name + '.migrate.lock')
    if lock.is_symlink():
        raise BuildctlError('migration lock uses a symlink')
    with lock.open('a') as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise BuildctlError('another migration is in progress') from error
        yield


class Migrator:
    def __init__(self, source, target, *, source_prefix=None, projects_dir=None,
                 worktrees=None, states=None, rehearsal=False):
        self.source, self.target = Path(source), Path(target)
        self.source_prefix = Path(source_prefix or source)
        self.projects_dir = Path(projects_dir or source.parent / 'Projects')
        self.worktrees = Path(worktrees or self.projects_dir / '.build-worktrees')
        self.states = states or (lambda: {'prod': 'unknown', 'dev': 'unknown'})
        self.rehearsal = rehearsal
        self.runner = Runner()

    @property
    def paths(self):
        return Paths(self.source_prefix, self.target)

    def stopped(self, states):
        if any(states.get(name) not in {'inactive', 'failed'} for name in ('prod', 'dev')):
            raise BuildctlError('both prod and dev units must be stopped (inactive or failed)')

    def preflight(self, force):
        no_symlinks(self.source)
        no_symlinks(self.target, tree=True)
        if self.source.is_relative_to(self.target) or self.target.is_relative_to(self.source):
            raise BuildctlError('migration source and target must be disjoint')
        for root in (self.projects_dir, self.worktrees):
            if root.is_relative_to(self.source_prefix) or root.is_relative_to(self.target):
                raise BuildctlError('shared roots must be outside both state roots')
        if self.target.exists() and not force:
            if any(path.name not in OPERATIONAL for path in self.target.iterdir()):
                raise BuildctlError('target is not fresh; use --force to retain a rollback backup')
        if not (self.source / 'tasks/build.db').is_file():
            raise BuildctlError('source tasks database is missing')

    def inventory(self):
        expected = scan_database(self.source / 'tasks/build.db', self.paths)
        self.lifecycle(expected)
        scratch = set()
        historical = []
        for value in expected['terminal_operation_roots']:
            mapped = Path(self.paths.path(value))
            relative = scratch_directory(value, self.source_prefix)
            actual = self.source / relative if relative else mapped
            if actual.is_dir():
                expected['references'].add(('root', value))
            else:
                historical.append(value)
        expected['historical_operation_roots'] = sorted(historical)
        for _, value in expected['references']:
            relative = scratch_directory(value, self.source_prefix)
            if relative:
                scratch.add(relative)
        entries = [Path(name) for name in IMPORTED if (self.source / name).exists()]
        entries += sorted(scratch)
        for relative in entries:
            path = self.source / relative
            no_symlinks(path, tree=True)
            if not path.exists():
                raise BuildctlError('referenced project-scratch directory is missing')
        return expected, entries

    def lifecycle(self, expected):
        workspaces = live_workspaces(self.worktrees)
        lifecycle = classify(self.source / 'tasks/build.db', live_workspaces=workspaces)
        historical = {}
        missing_paths = set()
        for kind, refs in lifecycle['historical'].items():
            missing = {value for _, value in refs if not self.original_path(value).exists()}
            historical[kind] = {'count': len(missing), 'sample': sorted(missing)[:10]}
            missing_paths.update(self.paths.path(value) for value in missing)
            expected['references'].difference_update(ref for ref in refs if ref[1] in missing)
        for workspace in workspaces:
            expected['references'].update((key, value) for key, value in references(workspace)
                                          if key in {'root', 'path'})
        expected['undeliverable_stale_operations'] = lifecycle.get('undeliverable_stale_operations', [])
        expected['undeliverable_stale_deliveries'] = lifecycle.get('undeliverable_stale_deliveries', [])
        expected['historical_missing_paths'] = missing_paths
        expected['historical_worktrees'] = historical
        expected['live_workspaces'] = len(workspaces)

    def original_path(self, value):
        mapped = Path(self.paths.path(value))
        if mapped.is_relative_to(self.target):
            return self.source / mapped.relative_to(self.target)
        return mapped

    def report(self, expected, entries, states):
        attachments = self.source / 'tasks/attachments'
        count = sum(1 for p in attachments.rglob('*') if p.is_file())
        rewrites = expected['rewrites']
        return {'tables': expected['tables'], 'rewrites': rewrites,
                'rewritten_values': sum(sum(cols.values()) for cols in rewrites.values()),
                'attachments': count, 'units': states,
                'entries': [str(p) for p in entries], 'verified': False,
                'rehearsal': self.rehearsal,
                'historical_operation_roots': {
                    'count': len(expected['historical_operation_roots']),
                    'sample': expected['historical_operation_roots'][:10]},
                'historical_worktrees': expected['historical_worktrees'],
                'live_workspaces': expected['live_workspaces'],
                'undeliverable_stale_operations': expected['undeliverable_stale_operations'],
                'undeliverable_stale_deliveries': expected['undeliverable_stale_deliveries']}

    def run(self, *, dry_run=False, force=False, prod_stopped=False):
        try:
            return self._run(dry_run=dry_run, force=force, prod_stopped=prod_stopped)
        except sqlite3.Error as error:
            raise BuildctlError('migration SQLite operation failed') from error

    def _run(self, *, dry_run, force, prod_stopped):
        self.preflight(force)
        states = self.states()
        if not dry_run:
            if not prod_stopped:
                raise BuildctlError('apply requires --prod-stopped assertion')
            self.stopped(states)
        expected, entries = self.inventory()
        result = self.report(expected, entries, states)
        result.update(self.disk_space(entries))
        expected['git_changes'] = self.plan_git(expected, result)
        if dry_run:
            result['worktrees'] = self.verify_paths(self.source, expected)
            return result
        with migration_lock(self.target):
            self.preflight(force)
            self.stopped(self.states())
            return self.stage(expected, entries, result)

    def plan_git(self, expected, result):
        changes = plan_remotes(self.source / 'tasks/build.db', self.source_prefix,
                               self.target, self.runner)
        current, historical = [], []
        for change in changes:
            change['repository'] = self.paths.path(change['repository'])
            if change['repository'] in expected['historical_missing_paths']:
                historical.append(change)
            else:
                current.append(change)
        result['shared_git_edits'] = len(current)
        result['historical_git_edits'] = {'count': len(historical), 'sample': historical[:10]}
        return current

    def disk_space(self, entries):
        size = sum(p.stat().st_size for entry in entries for p in files(self.source / entry))
        size += sum(p.stat().st_size for p in files(self.target))
        ancestor = self.target.parent
        while not ancestor.exists():
            ancestor = ancestor.parent
        free = shutil.disk_usage(ancestor).free
        required = size * 2 + 16 * 1024 * 1024
        if free < required:
            raise BuildctlError('insufficient free disk space for staging and rollback')
        return {'required_bytes': required, 'free_bytes': free}

    def stage(self, expected, entries, result):
        stage = Path(tempfile.mkdtemp(prefix='.' + self.target.name + '.stage-', dir=self.target.parent))
        try:
            if self.target.exists():
                shutil.copytree(self.target, stage, dirs_exist_ok=True)
            self.copy_state(stage, entries)
            rewrite_database(stage / 'tasks/build.db', self.paths)
            rewrite_files(stage, entries, self.paths)
            roots = {'projects_dir': str(self.projects_dir), 'worktrees': str(self.worktrees)}
            (stage / 'migration-roots.json').write_text(json.dumps(roots))
            config_path = stage / 'config.json'
            if config_path.exists():
                config = json.loads(config_path.read_text())
                config['projects_dir'] = str(self.projects_dir)
                config_path.write_text(json.dumps(config))
            verify_database(stage / 'tasks/build.db', expected)
            result['worktrees'] = self.verify_paths(stage, expected)
            verify_attachments(self.source, stage)
            changes = expected['git_changes']
            result['shared_git_edits'] = len(changes)
            result['verified'] = True
            self.stopped(self.states())
            return self.publish(stage, changes, result)
        finally:
            remove(stage)

    def copy_state(self, stage, entries):
        for relative in entries:
            target = stage / relative
            remove(target)
            if relative == Path('tasks'):
                copy_tasks(self.source / relative, target)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                copy_entry(self.source / relative, target)

    def verify_paths(self, stage, expected):
        checked = set()
        for key, value in expected['references']:
            mapped = Path(self.paths.path(value))
            internal = mapped.is_relative_to(self.target)
            actual = stage / mapped.relative_to(self.target) if internal else mapped
            worktree = key in WORKTREE_KEYS or (key == 'path' and value.startswith(str(self.worktrees)))
            if worktree and not actual.is_dir():
                raise BuildctlError('referenced worktree or session checkout is missing: ' + str(mapped))
            if internal and not actual.exists():
                raise BuildctlError('migrated path or attachment reference is missing: ' + str(mapped))
            if worktree:
                checked.add(str(mapped))
        return len(checked)

    def publish(self, stage, changes, result):
        backup = self.target.with_name(self.target.name + '.backup-' + uuid.uuid4().hex)
        manifest = {'target': str(self.target), 'source': str(self.source_prefix),
                    'git': changes, 'rehearsal': self.rehearsal, 'status': 'prepared',
                    'target_existed': self.target.exists(), 'stage': str(stage),
                    'git_status': 'untouched'}
        metadata = Path(str(backup) + '.json')
        write_manifest(metadata, manifest)
        if self.target.exists():
            self.target.rename(backup)
        else:
            backup.mkdir(mode=0o700)
        try:
            stage.rename(self.target)
            manifest['status'] = 'published'
            write_manifest(metadata, manifest)
            if not self.rehearsal:
                apply_remotes(changes, self.runner, before_commit=lambda: git_started(metadata, manifest))
            manifest['status'] = 'applied'
            write_manifest(metadata, manifest)
        except Exception as error:
            # Keep every recovery input. Reverse accepts already-old or new
            # remote values, including interruption halfway through the edits.
            raise BuildctlError('cutover interrupted; recover with --rollback ' + str(backup)) from error
        result['backup'] = str(backup)
        return result

    def rollback(self, backup):
        backup = Path(backup)
        no_symlinks(backup, tree=True)
        no_symlinks(self.target, tree=True)
        if backup.parent != self.target.parent or not backup.name.startswith(self.target.name + '.backup-'):
            raise BuildctlError('rollback backup does not belong to this target')
        metadata = Path(str(backup) + '.json')
        no_symlinks(metadata)
        try:
            manifest = json.loads(metadata.read_text())
        except (OSError, ValueError) as error:
            raise BuildctlError('missing or invalid rollback manifest') from error
        if manifest.get('target') != str(self.target) or manifest.get('status') not in {'prepared', 'published', 'applied'}:
            raise BuildctlError('rollback manifest is not a pending migration for this target')
        with migration_lock(self.target):
            self.stopped(self.states())
            return self.restore(backup, metadata, manifest)

    def restore(self, backup, metadata, manifest):
        if not backup.is_dir():
            raise BuildctlError('rollback state backup is missing')
        retired = self.target.with_name(self.target.name + '.retired-' + uuid.uuid4().hex)
        if self.target.exists():
            if not manifest['rehearsal'] and manifest.get('git_status') != 'untouched':
                apply_remotes(manifest['git'], self.runner, reverse=True)
            self.target.rename(retired)
        backup.rename(self.target)
        if not manifest['target_existed']:
            remove(self.target)
        manifest['status'] = 'rolled_back'
        write_manifest(metadata, manifest)
        return {'restored': str(self.target), 'retired': str(retired)}


def live_workspaces(worktrees):
    if not worktrees.is_dir():
        raise BuildctlError('shared worktree registry root is missing')
    workspaces = []
    for manifest in sorted((worktrees / 'workspaces').glob('*/*/.build-workspace.json')):
        no_symlinks(manifest)
        try:
            record = json.loads(manifest.read_text())
            if not isinstance(record, dict):
                raise ValueError('workspace is not an object')
        except (OSError, ValueError) as error:
            raise BuildctlError('cannot read a shared workspace manifest') from error
        workspaces.append(record)
    return workspaces


def files(root):
    if root.is_file():
        return [root]
    return (p for p in root.rglob('*') if p.is_file())


def copy_tasks(source, target):
    target.mkdir(parents=True)
    for path in source.iterdir():
        if path.name not in {'build.db', 'build.db-wal', 'build.db-shm'}:
            copy_entry(path, target / path.name)
    backup_database(source / 'build.db', target / 'build.db')


def rewrite_files(stage, entries, paths):
    for entry in entries:
        for path in files(stage / entry):
            if 'attachments' in path.relative_to(stage).parts:
                continue
            if path.suffix in {'.json', '.jsonl'}:
                rewrite_json_file(path, paths)
            elif path.name == 'config' and 'review-receivers' in path.parts:
                rewrite_git_file(path, paths)


def rewrite_json_file(path, paths):
    original = path.read_text()
    try:
        if path.suffix == '.jsonl':
            lines = [json.loads(line) for line in original.splitlines()]
            changed = [paths.structured(line) for line in lines]
            output = ''.join(json.dumps(line, ensure_ascii=False) + '\n' for line in changed)
        else:
            lines = json.loads(original)
            changed = paths.structured(lines)
            output = json.dumps(changed, ensure_ascii=False)
    except ValueError as error:
        raise BuildctlError('invalid structured migration sidecar; source left unchanged') from error
    if changed != lines:
        path.write_text(output)


def rewrite_git_file(path, paths):
    # Git config values are structured by key; never replace arbitrary lines.
    lines = []
    for line in path.read_text().splitlines(keepends=True):
        key, separator, value = line.partition('=')
        if separator and key.strip().lower() in {'url', 'pushurl', 'worktree'}:
            old = value.strip().strip('"')
            new = paths.path(old)
            if old != new:
                line = key + '= ' + new + '\n'
        lines.append(line)
    path.write_text(''.join(lines))


def verify_attachments(source, target):
    original = source / 'tasks/attachments'
    for path in files(original):
        copied = target / 'tasks/attachments' / path.relative_to(original)
        if not copied.is_file() or digest(path) != digest(copied):
            raise BuildctlError('attachment hash verification failed')


def write_manifest(path, value):
    with tempfile.NamedTemporaryFile(mode='w', dir=path.parent, prefix='.migration-', delete=False) as stream:
        temporary = Path(stream.name)
        try:
            os.chmod(temporary, 0o600)
            json.dump(value, stream)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, path)
            descriptor = os.open(path.parent, os.O_DIRECTORY)
            try:
                os.fsync(descriptor)
            finally:
                os.close(descriptor)
        finally:
            temporary.unlink(missing_ok=True)


def git_started(metadata, manifest):
    manifest['git_status'] = 'started'
    write_manifest(metadata, manifest)
