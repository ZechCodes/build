//! SQL projection rules. Keep predicates aligned with tracker inbox semantics;
//! parity tests compare persisted rows with the existing pure read derivation.

pub(super) const TABLES: &str = r#"
CREATE TABLE IF NOT EXISTS tracker_task_summary_facts (
    task_id TEXT NOT NULL, id TEXT NOT NULL, ulid TEXT NOT NULL,
    at TEXT NOT NULL, ts INTEGER, unread INTEGER NOT NULL,
    activity INTEGER NOT NULL, watched INTEGER NOT NULL, done_move INTEGER NOT NULL,
    agent_comment INTEGER NOT NULL, mentions INTEGER NOT NULL,
    PRIMARY KEY(task_id,id)
);
CREATE INDEX IF NOT EXISTS tracker_summary_unread ON tracker_task_summary_facts(task_id,ulid) WHERE unread=1;
CREATE INDEX IF NOT EXISTS tracker_summary_activity ON tracker_task_summary_facts(task_id,ts) WHERE activity=1;
CREATE INDEX IF NOT EXISTS tracker_summary_watched ON tracker_task_summary_facts(task_id,ulid) WHERE watched=1;
CREATE INDEX IF NOT EXISTS tracker_summary_done ON tracker_task_summary_facts(task_id,at,id) WHERE done_move=1;
CREATE TABLE IF NOT EXISTS tracker_task_summaries (
    task_id TEXT PRIMARY KEY, project_key TEXT NOT NULL, number INTEGER NOT NULL,
    record TEXT NOT NULL CHECK(length(CAST(record AS BLOB)) <= 32768),
    assignment_target TEXT CHECK(assignment_target IS NULL OR length(CAST(assignment_target AS BLOB))<=1024),
    identities TEXT NOT NULL CHECK(length(CAST(identities AS BLOB))<=8192)
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
CREATE TRIGGER IF NOT EXISTS tracker_choices_insert AFTER INSERT ON tracker_task_filter_choices BEGIN
    INSERT INTO tracker_task_filter_values(project_key,dimension,choice_key,value,refs)
    VALUES(NEW.project_key,NEW.dimension,NEW.choice_key,NEW.value,1)
    ON CONFLICT(project_key,dimension,choice_key) DO UPDATE SET refs=refs+1,value=MIN(value,excluded.value);
END;
CREATE TRIGGER IF NOT EXISTS tracker_choices_delete AFTER DELETE ON tracker_task_filter_choices BEGIN
    UPDATE tracker_task_filter_values SET refs=refs-1,
        value=COALESCE((SELECT value FROM tracker_task_filter_choices WHERE project_key=OLD.project_key
            AND dimension=OLD.dimension AND choice_key=OLD.choice_key ORDER BY value LIMIT 1),value)
        WHERE project_key=OLD.project_key AND dimension=OLD.dimension AND choice_key=OLD.choice_key;
    DELETE FROM tracker_task_filter_values WHERE project_key=OLD.project_key AND dimension=OLD.dimension
        AND choice_key=OLD.choice_key AND refs<=0;
END;
"#;

pub(super) fn insert_facts(table: &str, predicate: &str) -> String {
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
    format!("INSERT OR REPLACE INTO tracker_task_summary_facts(task_id,id,ulid,at,ts,unread,activity,watched,done_move,agent_comment,mentions)
        SELECT task_id,id,CASE WHEN instr(id,'-')>0 THEN substr(id,instr(id,'-')+1) ELSE id END,
            {at},CAST(ROUND((julianday({at})-2440587.5)*86400000) AS INTEGER),
            COALESCE(({unread}),0),COALESCE(({activity}),0),COALESCE(({watched}),0),COALESCE(({done_move}),0),COALESCE(({agent_comment}),0),COALESCE(({mentions}),0)
        FROM {table} WHERE {predicate};")
}

/// Preserve valid bounded fields; legacy oversized strings are capped and
/// clearly marked. Raw bodies, full link collections and history never enter
/// the card table. Actor ids are structural references with their own cap.
fn base_record() -> String {
    let actor = |field: &str| {
        format!(
            "CASE WHEN json_type(t.record,'$.{field}')='object' THEN json_object(
        'kind',substr(json_extract(t.record,'$.{field}.kind'),1,32),
        'agent_id',substr(json_extract(t.record,'$.{field}.agent_id'),1,128)) END"
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
        'attachment_count',COALESCE(json_array_length(t.record,'$.attachments'),0),
        'read_through',substr(json_extract(t.record,'$.read_through'),1,128),
        'watch_started_after',substr(json_extract(t.record,'$.watch_started_after'),1,128),
        'dismissed_through',substr(json_extract(t.record,'$.dismissed_through'),1,128),
        'dependencies',{dependencies},
        '_summary_invalid_refs',json(CASE WHEN length(CAST(t.id AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.read_through') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.watch_started_after') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.dismissed_through') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.assignee.agent_id') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.created_by.agent_id') AS BLOB))>128 OR
            length(CAST(json_extract(t.record,'$.links.parent_task_id') AS BLOB))>128 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.trackers') WHERE length(CAST(value AS BLOB))>128) OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.links.workspace_ids') WHERE length(CAST(value AS BLOB))>128)
            THEN 'true' ELSE 'false' END),
        'created_at',substr(t.created_at,1,64),'updated_at',substr(t.updated_at,1,64),
        'closed_at',substr(json_extract(t.record,'$.closed_at'),1,64),
        'done_at',CASE WHEN t.status='done' THEN substr(COALESCE(json_extract(t.record,'$.done_at'),
            (SELECT at FROM tracker_task_summary_facts WHERE task_id=t.id AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1),t.created_at),1,64) END,
        'watched',json(CASE WHEN json_extract(t.record,'$.watched')=1 THEN 'true' ELSE 'false' END),
        '_summary_invalid_labels',json(CASE WHEN json_array_length(t.record,'$.labels')>20 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.labels') WHERE length(CAST(value AS BLOB))>40)
            THEN 'true' ELSE 'false' END),
        'summary_truncated',json(CASE WHEN length(json_extract(t.record,'$.title'))>1024 OR
            json_array_length(t.record,'$.labels')>20 OR length(t.status)>128 OR
            json_array_length(t.record,'$.trackers')>32 OR json_array_length(t.record,'$.links.workspace_ids')>20 OR
            EXISTS(SELECT 1 FROM json_each(t.record,'$.labels') WHERE length(CAST(value AS BLOB))>40)
            THEN 'true' ELSE 'false' END))"#,
        assignee = actor("assignee"),
        creator = actor("created_by"),
        dependencies = dependencies("t.id")
    )
}

fn identities() -> &'static str {
    // A task may retain identities from its entire timeline. Only references
    // present on this card are projected; the timeline dictionary stays behind.
    r#"COALESCE((SELECT json_group_object(key,json(value)) FROM (SELECT i.key,json_object(
        'agent_id',substr(i.key,1,128),'name',substr(json_extract(i.value,'$.name'),1,128),
        'project_role',substr(json_extract(i.value,'$.project_role'),1,64),
        'ordinal',json_extract(i.value,'$.ordinal'),
        'workspace_id',substr(json_extract(i.value,'$.workspace_id'),1,128),
        'workspace_name',substr(json_extract(i.value,'$.workspace_name'),1,128),
        'provider',substr(json_extract(i.value,'$.provider'),1,64),
        'available',json(CASE WHEN json_extract(i.value,'$.available')=1 THEN 'true' ELSE 'false' END)) value
        FROM json_each(t.record,'$.identities') i
        WHERE length(CAST(i.key AS BLOB))<=128 AND i.key IN (json_extract(t.record,'$.created_by.agent_id'),json_extract(t.record,'$.assignee.agent_id'))
        LIMIT 2)), '{}')"#
}

fn stats_patch() -> &'static str {
    r#"CASE WHEN json_extract(t.record,'$.watched')=1 THEN json_object(
        'unread_agent_comments',(SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=t.id AND f.agent_comment=1
            AND f.ulid>COALESCE(substr(json_extract(t.record,'$.read_through'),instr(json_extract(t.record,'$.read_through'),'-')+1),'')),
        'unread_mentions',(SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=t.id AND f.mentions=1
            AND f.ulid>COALESCE(substr(json_extract(t.record,'$.read_through'),instr(json_extract(t.record,'$.read_through'),'-')+1),'')),
        'unread_count',(SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=t.id AND f.unread=1
            AND f.ulid>COALESCE(substr(json_extract(t.record,'$.read_through'),instr(json_extract(t.record,'$.read_through'),'-')+1),'')
            AND f.ulid>COALESCE(substr(json_extract(t.record,'$.watch_started_after'),instr(json_extract(t.record,'$.watch_started_after'),'-')+1),
                (SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=t.id AND watched=1),'')),
        'session_started_ms',(SELECT COALESCE(MAX(CASE WHEN prev IS NOT NULL AND ts-prev>=28800000 THEN ts END),
            json_extract(t.record,'$.activity_session.session_started_ms'),MIN(ts)) FROM (
            SELECT ts,LAG(ts) OVER (ORDER BY ts) prev FROM (
                SELECT json_extract(t.record,'$.activity_session.last_activity_ms') ts
                UNION SELECT ts FROM tracker_task_summary_facts WHERE task_id=t.id AND activity=1
                    AND ts>COALESCE(json_extract(t.record,'$.activity_session.last_activity_ms'),-9223372036854775808)
            ) WHERE ts IS NOT NULL)),
        'last_activity_ms',(SELECT MAX(ts) FROM (
            SELECT json_extract(t.record,'$.activity_session.last_activity_ms') ts
            UNION ALL SELECT ts FROM tracker_task_summary_facts WHERE task_id=t.id AND activity=1
                AND ts>COALESCE(json_extract(t.record,'$.activity_session.last_activity_ms'),-9223372036854775808)))
        ) ELSE json('{}') END"#
}

fn enrich_patch() -> &'static str {
    r#"json_patch(
        COALESCE((SELECT json_object('review_summary',json_object('task_id',task_id,
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

pub(super) fn refresh(id: &str) -> String {
    format!("INSERT INTO tracker_task_summaries(task_id,project_key,number,record,assignment_target,identities)
        SELECT t.id,t.project_key,t.number,json_patch(json_patch({base},{stats}),{enrich}),
            (SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN json_object('kind',substr(json_extract(record,'$.target.kind'),1,32),'agent_id',substr(json_extract(record,'$.target.agent_id'),1,128)) END FROM tracker_delivery_intents WHERE task_id=t.id
                AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),{identities}
        FROM tracker_tasks t WHERE t.id={id}
        ON CONFLICT(task_id) DO UPDATE SET project_key=excluded.project_key,number=excluded.number,
            record=excluded.record,assignment_target=excluded.assignment_target,identities=excluded.identities;
        {choices}", base=base_record(), stats=stats_patch(), enrich=enrich_patch(), identities=identities(), choices=refresh_choices(id))
}

fn refresh_choices(id: &str) -> String {
    format!(
        r#"DELETE FROM tracker_task_filter_choices WHERE task_id={id};
        INSERT OR IGNORE INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value)
            SELECT t.project_key,'label',lower(j.value),t.id,json_quote(j.value)
            FROM tracker_tasks t,json_each(t.record,'$.labels') j WHERE t.id={id};
        INSERT OR IGNORE INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value)
            SELECT project_key,'status',status,id,json_quote(status) FROM tracker_tasks WHERE id={id};
        INSERT OR IGNORE INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value)
            SELECT project_key,'state',state,id,json_quote(state) FROM tracker_tasks WHERE id={id};
        INSERT OR IGNORE INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value)
            SELECT project_key,'assignee',
                CASE json_extract(record,'$.assignee.kind') WHEN 'user' THEN 'user' WHEN 'project_agent' THEN 'project_agent'
                    WHEN 'agent' THEN json_extract(record,'$.assignee.agent_id') ELSE 'none' END,
                id,json_quote(CASE json_extract(record,'$.assignee.kind') WHEN 'user' THEN 'user' WHEN 'project_agent' THEN 'project_agent'
                    WHEN 'agent' THEN json_extract(record,'$.assignee.agent_id') ELSE 'none' END)
            FROM tracker_tasks WHERE id={id};"#
    )
}

pub(super) fn triggers() -> String {
    let mut sql = task_triggers();
    for table in ["tracker_comments", "tracker_events"] {
        sql.push_str(&history_triggers(table));
    }
    for table in ["reviews", "tracker_delivery_intents"] {
        sql.push_str(&metadata_triggers(table));
    }
    sql.push_str(&edge_triggers());
    sql
}

fn task_triggers() -> String {
    let mut sql = String::new();
    for operation in ["INSERT", "UPDATE"] {
        let task = refresh("NEW.id");
        let dependents=refresh_dependencies("task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.id)");
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_task_{operation} AFTER {operation} ON tracker_tasks BEGIN {task}{dependents} END;"));
    }
    sql.push_str("CREATE TRIGGER IF NOT EXISTS tracker_summary_task_delete AFTER DELETE ON tracker_tasks BEGIN
        DELETE FROM tracker_task_summaries WHERE task_id=OLD.id;
        DELETE FROM tracker_task_summary_facts WHERE task_id=OLD.id;
        DELETE FROM tracker_task_filter_choices WHERE task_id=OLD.id; END;");
    sql
}

fn history_triggers(table: &str) -> String {
    let mut sql = String::new();
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let prefix = if operation == "DELETE" { "OLD" } else { "NEW" };
        let id = format!("{prefix}.task_id");
        let insert = if operation == "DELETE" {
            String::new()
        } else {
            insert_facts(table, "id=NEW.id")
        };
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_{operation} AFTER {operation} ON {table} BEGIN
            DELETE FROM tracker_task_summary_facts WHERE task_id={id} AND id={prefix}.id;
            {insert}{refresh} END;",refresh=refresh(&id)));
    }
    sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_reparent AFTER UPDATE OF task_id ON {table}
        WHEN OLD.task_id<>NEW.task_id BEGIN DELETE FROM tracker_task_summary_facts WHERE task_id=OLD.task_id AND id=OLD.id; {} END;",refresh("OLD.task_id")));
    sql
}

fn metadata_triggers(table: &str) -> String {
    let mut sql = String::new();
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let id = if operation == "DELETE" {
            "OLD.task_id"
        } else {
            "NEW.task_id"
        };
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_{operation} AFTER {operation} ON {table} BEGIN {} END;",refresh(id)));
    }
    sql
}

fn edge_triggers() -> String {
    let mut sql = String::new();
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let predicate = match operation {
            "INSERT" => "task_id IN (NEW.task_id,NEW.waiting_on_task_id)",
            "DELETE" => "task_id IN (OLD.task_id,OLD.waiting_on_task_id)",
            _ => {
                "task_id IN (NEW.task_id,NEW.waiting_on_task_id,OLD.task_id,OLD.waiting_on_task_id)"
            }
        };
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_edges_{operation} AFTER {operation} ON tracker_task_dependencies BEGIN {} END;",refresh_dependencies(predicate)));
    }
    sql
}

fn dependencies(id: &str) -> String {
    format!("json_object('blocked',json(CASE WHEN (SELECT state='open' AND status<>'done' FROM tracker_tasks WHERE id={id}) AND
        EXISTS(SELECT 1 FROM tracker_task_dependencies d JOIN tracker_tasks b ON b.id=d.waiting_on_task_id
            WHERE d.task_id={id} AND b.state<>'closed' AND b.status<>'done') THEN 'true' ELSE 'false' END),
        'waiting_on_count',(SELECT COUNT(*) FROM tracker_task_dependencies WHERE task_id={id}),
        'blocks_count',(SELECT COUNT(*) FROM tracker_task_dependencies WHERE waiting_on_task_id={id}))")
}

fn refresh_dependencies(predicate: &str) -> String {
    format!("UPDATE tracker_task_summaries SET record=json_set(record,'$.dependencies',{}) WHERE {predicate};",dependencies("tracker_task_summaries.task_id"))
}
