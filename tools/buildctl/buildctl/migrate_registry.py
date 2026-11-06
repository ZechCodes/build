"""Registry-based checkout and receiver evidence, without filesystem reads."""
from pathlib import Path

from .migrate_paths import references

RECEIVER_KEYS = {'receiving_repository', 'common_git_dir', 'source_common_git_dir',
                 'root', 'path', 'cwd', 'checkout', 'checkout_path', 'worktree_path', 'working_repository'}

RECEIVER_POLICY = (
    'Missing receivers are historical when no live workspace uses them, or all linked PRs are merged. '
    'Closed/rejected PRs with live workspaces retain receivers, as does any direct live manifest reference. '
    'Pending openings, captures and unsettled merges/publications retain live receivers. '
    'Any live use of a shared receiver wins; existing receivers still verify normally.'
)


def is_live_path(value, live_paths):
    return isinstance(value, str) and any(root and (value == root or value.startswith(root + '/'))
                                          for root in live_paths)


def workspace_root(value, worktrees):
    """Only a full shared workspace folder proves deletion, never a missing child."""
    path = Path(value)
    try:
        parts = path.relative_to(worktrees / 'workspaces').parts
    except ValueError:
        return None
    if len(parts) < 2 or '..' in parts:
        return None
    return worktrees / 'workspaces' / parts[0] / parts[1]


def receiver_root(value, roots):
    for root in roots:
        if not value.startswith(root + '/'):
            continue
        name = value[len(root) + 1:]
        if '/' not in name and name.endswith('.git'):
            return root + '/' + name
    return None


def receiver_reference(ref, roots):
    key, value = ref
    return receiver_root(value, roots) if key in RECEIVER_KEYS else None


def workspace_id(raw, record):
    return record.get('workspace_id', raw.get('workspace_id')) or (record.get('request') or {}).get('workspace_id')


def entry_refs(raw, record):
    return set(references(record)) | raw['_refs']


def live_review_tasks(entries, live_ids, live_paths):
    tasks, merged = set(), set()
    for table, raw, record in entries:
        task = record.get('task_id', raw.get('task_id'))
        if table == 'reviews' and (record.get('pull_request') or {}).get('status') == 'merged':
            merged.add(task)
        if table == 'tracker_tasks' and live_ids.intersection(record.get('links', {}).get('workspace_ids', [])):
            tasks.add(record.get('id'))
        if table.startswith('review'):
            uses_live = workspace_id(raw, record) in live_ids
            uses_live |= any(is_live_path(value, live_paths) for _, value in entry_refs(raw, record))
            if uses_live:
                tasks.add(task)
    return tasks - {None}, merged - {None}


def historical_receivers(entries, live, live_paths, roots):
    """A receiver is shared: one retained live use vetoes all historical uses."""
    live_ids = {record.get('id') for record in live}
    live_tasks, merged = live_review_tasks(entries, live_ids, live_paths)
    merged.difference_update(recovery_tasks(entries))
    candidates, required = set(), set()
    for record in live:
        required.update(receiver_root(value, roots) for _, value in references(record))
    for table, raw, record in entries:
        receivers = {receiver_reference(ref, roots) for ref in entry_refs(raw, record)} - {None}
        candidates.update(receivers)
        task = record.get('task_id', raw.get('task_id'))
        direct_live = workspace_id(raw, record) in live_ids
        retained_task = task in live_tasks and task not in merged
        # Failed/interrupted openings still own recovery even for a prior merged PR.
        opening = table == 'review_openings' and record.get('state') in {'preparing', 'failed', 'interrupted'}
        if retained_task or (direct_live and (task not in merged or opening)):
            required.update(receivers)
    return candidates - required


def push_completed(source, intent, actions):
    push = source['push']
    return any(action.get('id') in intent.get('action_ids', [])
               and action.get('directory_id') == source.get('directory_id')
               and step.get('kind') == 'push' and step.get('status') == 'succeeded'
               and step.get('remote') == push.get('remote') and step.get('branch') == push.get('branch')
               for action in actions for step in action.get('steps', []))


def recovery_tasks(entries):
    """Conservatively retain non-succeeded merges and unproven push publications."""
    actions = {}
    for table, raw, record in entries:
        if table == 'review_actions':
            actions.setdefault(record.get('task_id', raw.get('task_id')), []).append(record)
    pending = set()
    for table, raw, record in entries:
        task = record.get('task_id', raw.get('task_id'))
        if table == 'review_sync_candidates':
            pending.add(task)
        if table != 'review_merge_intents':
            continue
        sources = (record.get('request') or {}).get('sources', [])
        unpublished = any(source.get('push') and not push_completed(source, record, actions.get(task, []))
                          for source in sources)
        if record.get('state', raw.get('state')) != 'succeeded' or unpublished:
            pending.add(task)
    return pending
