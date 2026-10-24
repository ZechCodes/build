"""Classify checkout references using persisted bridge lifecycle evidence.

Workspace manifests are the bridge's live registry, supplied by the caller.
Reclaim eligibility is deliberately ignored: it does not prove deletion.
Unknown records remain required, and any live use wins over historical uses.
"""
import json

from .migrate_paths import PROSE_KEYS, references
from .migrate_store import classified_references, connect, rows, table_names

HISTORY_KINDS = ('reclaimed_workspaces', 'finished_runs', 'closed_tasks',
                 'session_lineage', 'archived_worktrees')
CHECKOUT_KEYS = {'worktree_path', 'checkout', 'cwd', 'root', 'path', 'workspace_path', 'working_repository'}
TERMINAL_RUNS = {'Merged', 'Abandoned', 'Archived'}


def objects(value):
    """Walk structured records without interpreting prose or embedded JSON text."""
    if isinstance(value, dict):
        yield value
        for key, item in value.items():
            if key not in PROSE_KEYS:
                yield from objects(item)
    elif isinstance(value, list):
        for item in value:
            yield from objects(item)


def records(database):
    with connect(database) as db:
        for table in table_names(db):
            for columns, _, values in rows(db, table):
                raw = dict(zip(columns, values))
                value = raw.get('record')
                if isinstance(value, str):
                    value = json.loads(value)
                # Keep path provenance, not full transcript bodies, in the index.
                refs = set()
                for column, cell in raw.items():
                    if column != 'record':
                        refs.update(classified_references(table, column, raw.get('status'), cell)[0])
                retained = {key: raw[key] for key in ('id', 'task_id', 'status') if key in raw}
                retained['_refs'] = refs
                yield table, retained, value if isinstance(value, dict) else {}


def workspace_paths(workspace):
    return {value for key, value in references(workspace) if key in CHECKOUT_KEYS}


def reclaimed_evidence(entries):
    ids, paths = set(), set()
    for table, _, record in entries:
        if table == 'tracker_events' and record.get('kind') == 'workspace_reclaimed':
            payload = record.get('payload', {})
            ids.add(payload.get('workspace_id'))
            paths.update(workspace_paths(payload))
    return ids - {None}, paths


def record_kind(table, record, reclaimed_ids):
    identity = record.get('workspace_id', record.get('entity_id'))
    if identity in reclaimed_ids:
        return 'reclaimed_workspaces'
    if table == 'implementations' and record.get('state') in TERMINAL_RUNS:
        return 'finished_runs'
    if table == 'tracker_tasks' and (record.get('state') == 'closed' or record.get('status') == 'done'):
        return 'closed_tasks'
    if table == 'archived_worktrees':
        return 'archived_worktrees'
    return None


def lineage_kind(value, inherited, reclaimed_ids, closed_entities):
    if value.get('entity_id') in reclaimed_ids:
        return 'reclaimed_workspaces'
    if value.get('entity_id') in closed_entities:
        return 'closed_tasks'
    if value.get('ended_at') and 'checkout' in value:
        return 'session_lineage'
    return inherited


def structured_refs(value, inherited, reclaimed_ids, closed_entities):
    """A lineage's end marker applies only to that lineage's checkout and cwd."""
    if isinstance(value, dict):
        kind = lineage_kind(value, inherited, reclaimed_ids, closed_entities)
        for key, item in value.items():
            if key in PROSE_KEYS:
                continue
            if isinstance(item, str):
                for ref in references({key: item}):
                    yield ref, kind if key in CHECKOUT_KEYS else None
            else:
                yield from structured_refs(item, kind, reclaimed_ids, closed_entities)
    elif isinstance(value, list):
        for item in value:
            yield from structured_refs(item, inherited, reclaimed_ids, closed_entities)


def closed_task_evidence(entries):
    closed_tasks, linked = set(), {}
    for table, _, record in entries:
        if table != 'tracker_tasks':
            continue
        finished = record.get('state') == 'closed' or record.get('status') == 'done'
        if finished:
            closed_tasks.add(record.get('id'))
        for identity in record.get('links', {}).get('conversation_ids', []):
            linked.setdefault(identity, []).append(finished)
    return closed_tasks, {identity for identity, states in linked.items() if all(states)}


def classified_entry(table, raw, record, reclaimed_ids, closed_tasks, closed_entities):
    kind = record_kind(table, record, reclaimed_ids)
    if table == 'implementations' and record.get('id') in closed_entities:
        kind = 'closed_tasks'
    if table.startswith('review') and record.get('task_id', raw.get('task_id')) in closed_tasks:
        kind = 'closed_tasks'
    classified = list(structured_refs(record, kind, reclaimed_ids, closed_entities))
    known = {ref for ref, _ in classified}
    classified.extend((ref, kind if ref[0] in CHECKOUT_KEYS else None)
                      for ref in raw['_refs'] if ref not in known)
    return classified


def classify(database, live_workspaces=()):
    """Return required refs and historical refs grouped by evidence kind.

    Sets contain ``(field_name, original_path)`` pairs compatible with the
    migration store scanner. No filesystem path is tested by this module.
    """
    entries = list(records(database))
    live = list(live_workspaces) + [record for table, _, record in entries if table == 'workspaces']
    live_paths = set().union(*(workspace_paths(record) for record in live)) if live else set()
    reclaimed_ids, reclaimed_paths = reclaimed_evidence(entries)
    reclaimed_ids.difference_update(record.get('id') for record in live)
    closed_tasks, closed_entities = closed_task_evidence(entries)
    history = {kind: set() for kind in HISTORY_KINDS}
    required = set()
    for table, raw, record in entries:
        classified = classified_entry(table, raw, record, reclaimed_ids, closed_tasks, closed_entities)
        for ref, kind in classified:
            if ref[1] in live_paths:
                kind = None
            elif ref[0] in CHECKOUT_KEYS and ref[1] in reclaimed_paths:
                kind = 'reclaimed_workspaces'
            (history[kind] if kind else required).add(ref)
    required_paths = {value for _, value in required}
    for kind in history:
        history[kind] = {ref for ref in history[kind] if ref[1] not in required_paths}
    return {'required': required, 'historical': history}
