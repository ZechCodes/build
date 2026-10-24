"""Structured path mapping; prose and opaque attachment bytes are never rewritten."""

import json
from pathlib import Path
import re

PATH_KEYS = {
    'path', 'root', 'cwd', 'project_key', 'checkout', 'attachment', 'workspace',
    'working_repository', 'source_repository', 'receiving_repository',
    'source_common_git_dir', 'common_git_dir', 'projects_dir', 'worktrees',
}
JSON_COLUMNS = {'record', 'item', 'delivery'}
PROSE_KEYS = {'body', 'content', 'text', 'summary', 'message', 'prompt', 'description'}


def path_key(key):
    return key in PATH_KEYS or key.endswith(('_path', '_dir'))


class Paths:
    def __init__(self, source, target):
        self.source = str(source)
        self.target = str(target)

    def path(self, value):
        for prefix in (self.source, '~/.build'):
            if value == prefix or value.startswith(prefix + '/'):
                return self.target + value[len(prefix):]
        return value

    def structured(self, value, key=''):
        if isinstance(value, dict):
            return {name: self.structured(item, name) if name not in PROSE_KEYS else item
                    for name, item in value.items()}
        if isinstance(value, list):
            return [self.structured(item, key) for item in value]
        if isinstance(value, str) and path_key(key):
            return self.path(value)
        return value

    def cell(self, value, column):
        if not isinstance(value, str) or column in PROSE_KEYS:
            return value
        if self.source not in value and '~/.build' not in value:
            return value
        if column in JSON_COLUMNS and value.lstrip().startswith(('{', '[')):
            try:
                parsed = json.loads(value)
            except ValueError:
                return value
            changed = self.structured(parsed)
            return json.dumps(changed, ensure_ascii=False, separators=(',', ':')) if parsed != changed else value
        return self.path(value) if path_key(column) else value


def references(value, key=''):
    if isinstance(value, dict):
        for name, item in value.items():
            if name not in PROSE_KEYS:
                yield from references(item, name)
    elif isinstance(value, list):
        for item in value:
            yield from references(item, key)
    elif isinstance(value, str) and path_key(key):
        yield key, value


def scratch_directory(value, source):
    prefixes = (str(source) + '/project-scratch/', '~/.build/project-scratch/')
    prefix = next((item for item in prefixes if value.startswith(item)), None)
    if prefix is None:
        return None
    name = value[len(prefix):]
    if '/' in name:
        return None
    if not re.fullmatch(r'.+-[0-9a-f]{12}', name):
        from .runtime import BuildctlError
        raise BuildctlError('unsupported referenced project-scratch directory name')
    return Path('project-scratch') / name
