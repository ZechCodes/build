//! SQL projection rules. Keep predicates aligned with tracker inbox semantics;
//! parity tests compare persisted rows with the existing pure read derivation.

#[path = "sql/activity.rs"]
mod activity;
#[path = "sql/choices.rs"]
mod choices;

pub(super) const TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS tracker_task_summary_facts (
    task_id TEXT NOT NULL, id TEXT NOT NULL, ulid TEXT NOT NULL,
    at TEXT NOT NULL, ts INTEGER, unread INTEGER NOT NULL,
    activity INTEGER NOT NULL, meaningful INTEGER NOT NULL DEFAULT 0, watched INTEGER NOT NULL, done_move INTEGER NOT NULL,
    agent_comment INTEGER NOT NULL, mentions INTEGER NOT NULL,
    PRIMARY KEY(task_id,id)
);
CREATE INDEX IF NOT EXISTS tracker_summary_unread ON tracker_task_summary_facts(task_id,ulid) WHERE unread=1;
CREATE INDEX IF NOT EXISTS tracker_summary_activity_facts ON tracker_task_summary_facts(task_id,ts) WHERE activity=1;
CREATE INDEX IF NOT EXISTS tracker_summary_watched ON tracker_task_summary_facts(task_id,ulid) WHERE watched=1;
CREATE INDEX IF NOT EXISTS tracker_summary_done ON tracker_task_summary_facts(task_id,at,id) WHERE done_move=1;
CREATE TABLE IF NOT EXISTS tracker_task_summary_identities (
    agent_id TEXT PRIMARY KEY,
    record TEXT NOT NULL CHECK(length(CAST(record AS BLOB))<=4096),
    owner_worktree_path TEXT CHECK(length(CAST(owner_worktree_path AS BLOB))<=4096),
    owner_path_invalid INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS tracker_task_summaries (
    task_id TEXT PRIMARY KEY, project_key TEXT NOT NULL, number INTEGER NOT NULL,
    record TEXT NOT NULL CHECK(length(CAST(record AS BLOB)) <= 32768),
    assignment_target TEXT CHECK(assignment_target IS NULL OR length(CAST(assignment_target AS BLOB))<=1024),
    identities TEXT NOT NULL CHECK(length(CAST(identities AS BLOB))<=8192),
    context TEXT NOT NULL DEFAULT '{}' CHECK(length(CAST(context AS BLOB))<=4096)
);
CREATE TABLE IF NOT EXISTS tracker_task_summary_context (
    task_id TEXT PRIMARY KEY, watched INTEGER NOT NULL,
    read_ulid TEXT, watch_ulid TEXT, session_start INTEGER, session_last INTEGER,
    input_done_at TEXT, status TEXT NOT NULL, created_at TEXT NOT NULL,
    unread_count INTEGER NOT NULL DEFAULT 0, unread_agent_comments INTEGER NOT NULL DEFAULT 0,
    unread_mentions INTEGER NOT NULL DEFAULT 0, session_started_ms INTEGER, last_activity_ms INTEGER, done_at TEXT,
    CHECK(length(CAST(read_ulid AS BLOB))<=128), CHECK(length(CAST(watch_ulid AS BLOB))<=128),
    CHECK(length(CAST(input_done_at AS BLOB))<=64),CHECK(length(CAST(status AS BLOB))<=128),
    CHECK(length(CAST(created_at AS BLOB))<=64),CHECK(length(CAST(done_at AS BLOB))<=64)
);
CREATE INDEX IF NOT EXISTS tracker_summaries_number ON tracker_task_summaries(project_key,number DESC);
CREATE TABLE IF NOT EXISTS tracker_task_filter_choices (
    project_key TEXT NOT NULL, dimension TEXT NOT NULL, choice_key TEXT NOT NULL,
    task_id TEXT NOT NULL, value TEXT NOT NULL,
    PRIMARY KEY(project_key,dimension,choice_key,task_id)
);
CREATE INDEX IF NOT EXISTS tracker_choices_task ON tracker_task_filter_choices(task_id);
CREATE INDEX IF NOT EXISTS tracker_choices_spelling ON tracker_task_filter_choices(project_key,dimension,choice_key,value);
CREATE TABLE IF NOT EXISTS tracker_task_filter_values (
    project_key TEXT NOT NULL, dimension TEXT NOT NULL, choice_key TEXT NOT NULL,
    value TEXT NOT NULL, refs INTEGER NOT NULL,
    PRIMARY KEY(project_key,dimension,choice_key)
);
"#;

pub(super) fn fact_rows(table: &str) -> String {
    let (at, unread, activity, watched, done_move) = if table == "tracker_comments" {
        (
            "created_at",
            "json_extract(record,'$.author.kind') <> 'user'".to_string(),
            "1".to_string(),
            "0",
            "0",
        )
    } else {
        let kind = "json_extract(record,'$.kind')";
        let by = "COALESCE(json_extract(record,'$.payload.by'),'')";
        let dispatch_move = format!("({kind}='moved' AND {by} IN ('dispatch','report'))");
        let unread = format!("json_extract(record,'$.actor.kind') <> 'user' AND NOT {dispatch_move} AND
            ({kind} IN ('assigned','unassigned','moved','closed','reopened','review_completed') OR
             ({kind}='created' AND json_extract(record,'$.mentions_user')=1 AND json_extract(record,'$.actor.kind')='agent'))");
        let activity = format!("NOT ({kind} IN ('blocked','unblocked','dispatch_deferred','dispatch_failed','dispatched',
            'workspace_idle','workspace_pruned','workspace_reclaimed','branch_deleted','branch_kept') OR
            ({kind} IN ('linked','unlinked') AND json_type(record,'$.payload.waiting_on') IS NOT NULL) OR
            ({kind}='tracked' AND {by}='assignment') OR {dispatch_move} OR
            ({kind}='untracked' AND {by}='finished'))");
        (
            "at",
            unread,
            activity,
            "json_extract(record,'$.kind')='watched'",
            "json_extract(record,'$.kind')='moved' AND json_extract(record,'$.payload.to')='done'",
        )
    };
    let (agent_comment, mentions) = if table == "tracker_comments" {
        ("json_extract(record,'$.author.kind')='agent'", "json_extract(record,'$.author.kind')='agent' AND (json_extract(record,'$.mentions_user')=1 OR json_extract(record,'$.notifies_user')=1)")
    } else {
        ("0", "json_extract(record,'$.kind')='created' AND json_extract(record,'$.actor.kind')='agent' AND (json_extract(record,'$.mentions_user')=1 OR json_extract(record,'$.notifies_user')=1)")
    };
    let meaningful = if table == "tracker_comments" {
        "1".to_string()
    } else {
        "NOT (json_extract(record,'$.kind') IN ('blocked','unblocked','dispatch_deferred','dispatch_failed','dispatched') OR (json_extract(record,'$.kind') IN ('linked','unlinked') AND json_type(record,'$.payload.waiting_on') IS NOT NULL) OR (json_extract(record,'$.kind')='tracked' AND COALESCE(json_extract(record,'$.payload.by'),'')='assignment') OR (json_extract(record,'$.kind')='moved' AND COALESCE(json_extract(record,'$.payload.by'),'') IN ('dispatch','report')) OR (json_extract(record,'$.kind')='untracked' AND COALESCE(json_extract(record,'$.payload.by'),'')='finished'))".to_string()
    };
    format!("SELECT rowid source_rowid,task_id,id,CASE WHEN instr(id,'-')>0 THEN substr(id,instr(id,'-')+1) ELSE id END ulid,
            {at} at,{timestamp} ts,
            COALESCE(({unread}),0) unread,COALESCE(({activity}),0) activity,COALESCE(({meaningful}),0) meaningful,COALESCE(({watched}),0) watched,COALESCE(({done_move}),0) done_move,COALESCE(({agent_comment}),0) agent_comment,COALESCE(({mentions}),0) mentions
        FROM {table}",timestamp=timestamp_millis(at))
}

// Incremental facts, explicit repair and bounded startup backfill all use the
// same canonical Rust parser as legacy task activity/session reads.
fn timestamp_millis(at: &str) -> String {
    format!("task_summary_message_millis({at})")
}

/// Preserve valid bounded fields; legacy oversized strings are capped and
/// clearly marked. Raw bodies, full link collections and history never enter
/// the card table. Actor ids are structural references with their own cap.
fn base_record() -> String {
    let actor = |field: &str| {
        format!(
            "CASE WHEN json_type(t.record,'$.{field}')='object' THEN CASE WHEN json_extract(t.record,'$.{field}.kind')='agent'
            THEN json_object('kind','agent','agent_id',substr(json_extract(t.record,'$.{field}.agent_id'),1,128))
            ELSE json_object('kind',substr(json_extract(t.record,'$.{field}.kind'),1,32)) END END"
        )
    };
    format!(
        r#"json_object(
        'id',t.id,'number',t.number,'title',substr(json_extract(t.record,'$.title'),1,1024),
        'state',t.state,'status',substr(t.status,1,128),
        'labels',json(COALESCE((SELECT json_group_array(value) FROM
            (SELECT substr(value,1,40) value FROM json_each(t.record,'$.labels') LIMIT 20)),'[]')),
        'priority',substr(COALESCE(json_extract(t.record,'$.priority'),'none'),1,32),
        'assignee',json({assignee}),'created_by',json({creator}),
        'trackers',json(COALESCE((SELECT json_group_array(value) FROM
            (SELECT substr(value,1,128) value FROM json_each(t.record,'$.trackers') LIMIT 32)),'[]')),
        'links',json_object('parent_task_id',substr(json_extract(t.record,'$.links.parent_task_id'),1,128),
            'workspace_ids',json(COALESCE((SELECT json_group_array(value) FROM
                (SELECT substr(value,1,128) value FROM json_each(t.record,'$.links.workspace_ids') LIMIT 20)),'[]'))),
        'attachment_count',COALESCE(json_extract(t.record,'$.attachment_count'),json_array_length(t.record,'$.attachments'),0),
        'read_through',substr(json_extract(t.record,'$.read_through'),1,128),
        'watch_started_after',substr(json_extract(t.record,'$.watch_started_after'),1,128),
        'dismissed_through',substr(json_extract(t.record,'$.dismissed_through'),1,128),
        'dependencies',json(COALESCE((SELECT json_extract(s.record,'$.dependencies') FROM tracker_task_summaries s WHERE s.task_id=t.id),'{{"blocked":false,"waiting_on_count":0,"blocks_count":0}}')),
        '_summary_invalid_refs',json(CASE WHEN length(CAST(t.id AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.read_through') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.watch_started_after') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.dismissed_through') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.assignee.agent_id') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.created_by.agent_id') AS BLOB))>128 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.identities') i
                WHERE i.key IN (json_extract(t.record,'$.created_by.agent_id'),json_extract(t.record,'$.assignee.agent_id'))
                AND length(CAST(json_extract(i.value,'$.workspace_id') AS BLOB))>128) OR
            length(CAST(json_extract(t.record,'$.links.parent_task_id') AS BLOB))>128 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.trackers') WHERE length(CAST(value AS BLOB))>128) OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.links.workspace_ids') WHERE length(CAST(value AS BLOB))>128)
            THEN 'true' ELSE 'false' END),
        'created_at',substr(t.created_at,1,64),'updated_at',substr(t.updated_at,1,64),
        'closed_at',substr(json_extract(t.record,'$.closed_at'),1,64),
        'done_at',substr(json_extract(t.record,'$.done_at'),1,64),
        'watched',json(CASE WHEN json_extract(t.record,'$.watched')=1 THEN 'true' ELSE 'false' END),
        '_summary_invalid_labels',json(CASE WHEN json_array_length(t.record,'$.labels')>20 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.labels') WHERE type<>'text' OR value='' OR value<>trim(value) OR length(CAST(value AS BLOB))>40)
            THEN 'true' ELSE 'false' END),
        'summary_truncated',json(CASE WHEN length(json_extract(t.record,'$.title'))>1024 OR
            json_array_length(t.record,'$.labels')>20 OR length(t.status)>128 OR
            json_array_length(t.record,'$.trackers')>32 OR json_array_length(t.record,'$.links.workspace_ids')>20 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.labels') WHERE type<>'text' OR value='' OR value<>trim(value) OR length(CAST(value AS BLOB))>40)
            THEN 'true' ELSE 'false' END))"#,
        assignee = actor("assignee"),
        creator = actor("created_by"),
    )
}

fn identities() -> &'static str {
    // A task may retain identities from its entire timeline. Only references
    // present on this card are projected; the timeline dictionary stays behind.
    // Opaque navigation IDs are exact at the byte bound. Invalid IDs are left
    // out of this bounded record and base_record marks the whole card invalid;
    // readers must refuse it rather than publish an altered navigation target.
    r#"COALESCE((SELECT json_group_object(key,json(value)) FROM (SELECT i.key,json_object(
        'agent_id',substr(i.key,1,128),'name',substr(json_extract(i.value,'$.name'),1,128),
        'project_role',substr(json_extract(i.value,'$.project_role'),1,64),
        'ordinal',json_extract(i.value,'$.ordinal'),
        'workspace_id',CASE WHEN length(CAST(json_extract(i.value,'$.workspace_id') AS BLOB))<=128
            THEN json_extract(i.value,'$.workspace_id') END,
        'workspace_name',substr(json_extract(i.value,'$.workspace_name'),1,128),
        'provider',substr(json_extract(i.value,'$.provider'),1,64),
        'available',json(CASE WHEN json_extract(i.value,'$.available')=1 THEN 'true' ELSE 'false' END)) value
        FROM json_each(t.record,'$.identities') i
        WHERE length(CAST(i.key AS BLOB))<=128 AND i.key IN (json_extract(t.record,'$.created_by.agent_id'),json_extract(t.record,'$.assignee.agent_id'))
        LIMIT 2)), '{}')"#
}

fn enrich_patch() -> &'static str {
    r#"json_patch(
        COALESCE((SELECT json_object('review_summary',json_object('task_id',task_id,'mode','pull_request',
            'workspace_id',substr(json_extract(record,'$.pull_request.originating_workspace_id'),1,128),
            'version',version,'status',substr(json_extract(record,'$.pull_request.status'),1,64),
            'latest_published_snapshot_id',substr(json_extract(record,'$.pull_request.latest_published_snapshot_id'),1,128)))
            FROM reviews WHERE task_id=t.id AND json_extract(record,'$.mode')='pull_request'
                AND json_type(record,'$.pull_request')='object'),'{}'),
        COALESCE((SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN
            json_object('deferred_assignment',json_object('id',substr(id,1,128),
                'target',json_object('kind',substr(json_extract(record,'$.target.kind'),1,32),'agent_id',substr(json_extract(record,'$.target.agent_id'),1,128)),'state',CASE state WHEN 'provisioning' THEN 'queued' ELSE state END,
                'error',substr(json_extract(record,'$.error'),1,512))) ELSE '{}' END
            FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),'{}'))"#
}

const FACT_COLUMNS: &str =
    "task_id,id,ulid,at,ts,unread,activity,meaningful,watched,done_move,agent_comment,mentions";

fn fact_insert(table: &str, predicate: &str) -> String {
    format!("INSERT INTO tracker_task_summary_facts({FACT_COLUMNS}) SELECT {FACT_COLUMNS} FROM ({}) WHERE {}", fact_rows(table),predicate.replace("rowid","source_rowid"))
}

pub(super) fn insert_facts(table: &str, predicate: &str) -> String {
    let fields = [
        "ulid",
        "at",
        "ts",
        "unread",
        "activity",
        "meaningful",
        "watched",
        "done_move",
        "agent_comment",
        "mentions",
    ];
    let set = fields
        .iter()
        .map(|f| format!("{f}=excluded.{f}"))
        .collect::<Vec<_>>()
        .join(",");
    let changed = fields
        .iter()
        .map(|f| format!("{f} IS NOT excluded.{f}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!(
        "{} ON CONFLICT(task_id,id) DO UPDATE SET {set} WHERE {changed};",
        fact_insert(table, predicate)
    )
}

pub(super) fn refresh_task() -> String {
    format!("INSERT INTO tracker_task_summaries(task_id,project_key,number,record,assignment_target,identities,context)
        SELECT t.id,t.project_key,t.number,{base},NULL,{identities},{context}
        FROM (SELECT ?1 id,?2 project_key,?3 number,?4 state,?5 status,?6 created_at,?7 updated_at,jsonb(?8) record) t WHERE true
        ON CONFLICT(task_id) DO UPDATE SET project_key=excluded.project_key,number=excluded.number,
            record=excluded.record,assignment_target=NULL,identities=excluded.identities,context=excluded.context;",
        base=base_record(),identities=identities(),context=activity::context())
}

pub(super) fn refresh_context(id: &str) -> String {
    let inputs = [
        "watched",
        "read_ulid",
        "watch_ulid",
        "session_start",
        "session_last",
        "input_done_at",
        "status",
        "created_at",
    ];
    let changed = inputs
        .iter()
        .map(|field| format!("{field} IS NOT excluded.{field}"))
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("INSERT INTO tracker_task_summary_context(task_id,watched,read_ulid,watch_ulid,session_start,session_last,input_done_at,status,created_at)
        SELECT task_id,CASE WHEN json_extract(context,'$.watched')=1 THEN 1 ELSE 0 END,
            json_extract(context,'$.read_ulid'),json_extract(context,'$.watch_ulid'),json_extract(context,'$.session_start'),
            json_extract(context,'$.session_last'),json_extract(context,'$.done_at'),json_extract(context,'$.status'),json_extract(context,'$.created_at')
            FROM tracker_task_summaries WHERE task_id={id}
        ON CONFLICT(task_id) DO UPDATE SET watched=excluded.watched,read_ulid=excluded.read_ulid,watch_ulid=excluded.watch_ulid,
            session_start=excluded.session_start,session_last=excluded.session_last,input_done_at=excluded.input_done_at,status=excluded.status,created_at=excluded.created_at
        WHERE {changed};")
}

pub(super) fn refresh_metadata(id: &str) -> String {
    format!("UPDATE tracker_task_summaries SET (record,assignment_target)=(SELECT
        json_set(json_patch(json_remove(tracker_task_summaries.record,'$.review_summary','$.deferred_assignment'),m.patch),
            '$._summary_invalid_metadata_refs',json(CASE WHEN m.invalid_refs THEN 'true' ELSE 'false' END)),m.assignment_target
        FROM (SELECT t.task_id,{patch} patch,({invalid}) invalid_refs,
            (SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN
                json_object('kind',substr(json_extract(record,'$.target.kind'),1,32),'agent_id',substr(json_extract(record,'$.target.agent_id'),1,128)) END
                FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1) assignment_target
            FROM (SELECT task_id,task_id id FROM tracker_task_summaries WHERE task_id={id}) t) m
        WHERE m.task_id=tracker_task_summaries.task_id) WHERE task_id={id};",patch=enrich_patch(),invalid=invalid_metadata_refs())
}

fn invalid_metadata_refs() -> &'static str {
    r#"EXISTS(SELECT 1 FROM reviews WHERE task_id=t.id AND json_extract(record,'$.mode')='pull_request'
            AND json_type(record,'$.pull_request')='object'
            AND (length(CAST(json_extract(record,'$.pull_request.originating_workspace_id') AS BLOB))>128
                OR length(CAST(json_extract(record,'$.pull_request.latest_published_snapshot_id') AS BLOB))>128))
        OR COALESCE((SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN
            length(CAST(id AS BLOB))>128 OR length(CAST(json_extract(record,'$.target.agent_id') AS BLOB))>128 ELSE 0 END
            FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),0)"#
}

fn dependencies(id: &str) -> String {
    format!("json_object('blocked',json(CASE WHEN (SELECT state='open' AND status<>'done' FROM tracker_tasks WHERE id={id}) AND
        EXISTS(SELECT 1 FROM tracker_task_dependencies d JOIN tracker_tasks b ON b.id=d.waiting_on_task_id
            WHERE d.task_id={id} AND b.state<>'closed' AND b.status<>'done') THEN 'true' ELSE 'false' END),
        'waiting_on_count',(SELECT COUNT(*) FROM tracker_task_dependencies WHERE task_id={id}),
        'blocks_count',(SELECT COUNT(*) FROM tracker_task_dependencies WHERE waiting_on_task_id={id}))")
}

pub(super) fn refresh_dependencies(id: &str) -> String {
    format!("UPDATE tracker_task_summaries SET record=json_set(record,'$.dependencies',json({})) WHERE task_id={id}",dependencies(id))
}

fn identity_rows() -> &'static str {
    "SELECT a.rowid source_rowid,a.id,a.owner_id,json_object('agent_id',a.id,'name',substr(json_extract(a.record,'$.name'),1,128),
        'project_role',substr(json_extract(a.record,'$.project_role'),1,64),'ordinal',a.ordinal,
        'provider',substr(COALESCE(json_extract(a.record,'$.choice.provider'),'claude'),1,64),
        'workspace_id',NULL,'workspace_name',CASE WHEN json_extract(a.record,'$.project_role') IS NULL
            THEN substr(json_extract(r.record,'$.worktree_name'),1,128) END,'available',json('false')) record,
        CASE WHEN json_extract(a.record,'$.project_role') IS NULL
            AND json_type(r.record,'$.worktree_path')='text'
            AND length(CAST(json_extract(r.record,'$.worktree_path') AS BLOB))<=4096
            THEN json_extract(r.record,'$.worktree_path') END owner_worktree_path,
        CASE WHEN json_extract(a.record,'$.project_role') IS NULL AND
            (json_type(r.record,'$.worktree_path') NOT IN ('text','null')
             OR length(CAST(json_extract(r.record,'$.worktree_path') AS BLOB))>4096)
            THEN 1 ELSE 0 END owner_path_invalid
        FROM agents a LEFT JOIN implementations r ON r.id=a.owner_id WHERE length(CAST(a.id AS BLOB))<=128"
}

/// Owner metadata is parsed on writes only. Reads seek a bounded identity row.
pub(super) fn refresh_identities(predicate: &str) -> String {
    format!("INSERT INTO tracker_task_summary_identities(agent_id,record,owner_worktree_path,owner_path_invalid)
        SELECT a.id,a.record,a.owner_worktree_path,a.owner_path_invalid FROM ({rows}) a WHERE {predicate}
        ON CONFLICT(agent_id) DO UPDATE SET record=excluded.record,
            owner_worktree_path=excluded.owner_worktree_path,owner_path_invalid=excluded.owner_path_invalid
        WHERE record IS NOT excluded.record OR owner_worktree_path IS NOT excluded.owner_worktree_path
            OR owner_path_invalid IS NOT excluded.owner_path_invalid;",predicate=predicate.replace("a.rowid","a.source_rowid"),rows=identity_rows())
}

pub(super) use activity::{append_update, patch_card, refresh as refresh_activity};

pub(super) use choices::refresh as refresh_choices;
