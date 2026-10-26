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
    activity INTEGER NOT NULL, watched INTEGER NOT NULL, done_move INTEGER NOT NULL,
    agent_comment INTEGER NOT NULL, mentions INTEGER NOT NULL,
    PRIMARY KEY(task_id,id)
);
CREATE INDEX IF NOT EXISTS tracker_summary_unread ON tracker_task_summary_facts(task_id,ulid) WHERE unread=1;
CREATE INDEX IF NOT EXISTS tracker_summary_activity_facts ON tracker_task_summary_facts(task_id,ts) WHERE activity=1;
CREATE INDEX IF NOT EXISTS tracker_summary_watched ON tracker_task_summary_facts(task_id,ulid) WHERE watched=1;
CREATE INDEX IF NOT EXISTS tracker_summary_done ON tracker_task_summary_facts(task_id,at,id) WHERE done_move=1;
CREATE TABLE IF NOT EXISTS tracker_task_summary_identities (
    agent_id TEXT PRIMARY KEY,
    record TEXT NOT NULL CHECK(length(CAST(record AS BLOB))<=4096)
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

fn fact_view(table: &str) -> String {
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
    let name = if table == "tracker_comments" {
        "comment"
    } else {
        "event"
    };
    format!("CREATE VIEW IF NOT EXISTS tracker_summary_{name}_facts AS SELECT rowid source_rowid,task_id,id,CASE WHEN instr(id,'-')>0 THEN substr(id,instr(id,'-')+1) ELSE id END ulid,
            {at} at,{timestamp} ts,
            COALESCE(({unread}),0) unread,COALESCE(({activity}),0) activity,COALESCE(({watched}),0) watched,COALESCE(({done_move}),0) done_move,COALESCE(({agent_comment}),0) agent_comment,COALESCE(({mentions}),0) mentions
        FROM {table};",timestamp=timestamp_millis(at))
}

// SQLite's date parser rounds fractions to milliseconds. Rust's
// message_millis truncates nanoseconds, including towards zero before 1970.
// Parse a whole second and the decimal fraction separately to agree exactly.
fn timestamp_millis(at: &str) -> String {
    let zone = format!("CASE WHEN substr({at},-1)='Z' THEN 'Z' ELSE substr({at},-6) END");
    let second = format!("unixepoch(substr({at},1,19)||({zone}))");
    let fraction=format!("substr({at},instr({at},'.')+1,length({at})-instr({at},'.')-CASE WHEN substr({at},-1)='Z' THEN 1 ELSE 6 END)");
    format!(
        "CASE WHEN instr({at},'.')=0 THEN unixepoch({at})*1000 ELSE
        ({second})*1000+CAST(substr(({fraction})||'000',1,3) AS INTEGER)+
        CASE WHEN ({second})<0 AND substr(({fraction}),4) GLOB '*[1-9]*' THEN 1 ELSE 0 END END"
    )
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
        'attachment_count',COALESCE(json_array_length(t.record,'$.attachments'),0),
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

const FACT_COLUMNS: &str =
    "task_id,id,ulid,at,ts,unread,activity,watched,done_move,agent_comment,mentions";

fn fact_insert(table: &str, predicate: &str) -> String {
    let view = if table == "tracker_comments" {
        "tracker_summary_comment_facts"
    } else {
        "tracker_summary_event_facts"
    };
    format!("INSERT INTO tracker_task_summary_facts({FACT_COLUMNS}) SELECT {FACT_COLUMNS} FROM {view} WHERE {predicate}",predicate=predicate.replace("rowid","source_rowid"))
}

pub(super) fn insert_facts(table: &str, predicate: &str) -> String {
    let fields = [
        "ulid",
        "at",
        "ts",
        "unread",
        "activity",
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

pub(super) fn helpers() -> String {
    format!("{comment}{event}
        CREATE VIEW IF NOT EXISTS tracker_summary_task AS SELECT id task_id,project_key,number,state,status,created_at,updated_at,record FROM tracker_tasks;
        CREATE VIEW IF NOT EXISTS tracker_summary_metadata AS SELECT t.task_id,{patch} patch,({invalid}) invalid_refs,
            (SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN
                json_object('kind',substr(json_extract(record,'$.target.kind'),1,32),'agent_id',substr(json_extract(record,'$.target.agent_id'),1,128)) END
                FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1) assignment_target
            FROM (SELECT task_id,task_id id FROM tracker_task_summaries) t;
        CREATE VIEW IF NOT EXISTS tracker_summary_dependencies AS SELECT task_id,{dependencies} record FROM tracker_task_summaries;
        CREATE VIEW IF NOT EXISTS tracker_summary_identity AS {identity};
        {activity}{choices}",comment=fact_view("tracker_comments"),event=fact_view("tracker_events"),
        patch=enrich_patch(),invalid=invalid_metadata_refs(),dependencies=dependencies("tracker_task_summaries.task_id"),
        identity=identity_rows(),activity=activity::views(),choices=choices::view())
}

pub(super) fn rebuild_task(id: &str) -> String {
    format!(
        "{}{}{}{}",
        refresh_task(id),
        refresh_followup(id),
        activity::patch_card(id),
        refresh_dependencies(&format!("task_id={id}"))
    )
}

fn refresh_task(id: &str) -> String {
    format!("INSERT INTO tracker_task_summaries(task_id,project_key,number,record,assignment_target,identities,context)
        SELECT t.id,t.project_key,t.number,{base},NULL,{identities},{context}
        FROM (WITH parsed AS MATERIALIZED (SELECT task_id id,project_key,number,state,status,created_at,updated_at,jsonb(record) record
            FROM tracker_summary_task WHERE task_id={id}) SELECT * FROM parsed) t WHERE true
        ON CONFLICT(task_id) DO UPDATE SET project_key=excluded.project_key,number=excluded.number,
            record=excluded.record,assignment_target=NULL,identities=excluded.identities,context=excluded.context;",
        base=base_record(),identities=identities(),context=activity::context())
}

fn refresh_context(id: &str) -> String {
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

fn refresh_followup(id: &str) -> String {
    format!(
        "{}{}{}{}",
        refresh_context(id),
        activity::refresh(id),
        refresh_metadata(id),
        choices::refresh(id)
    )
}

fn apply(name: &str, id: &str) -> String {
    format!("INSERT INTO tracker_summary_apply_{name}(task_id) VALUES({id});")
}

// These empty views are synchronous SQL entry points, not stored work. Their
// persistent INSTEAD OF triggers run inside the canonical caller's statement,
// including independent writers; no callback registration or drain is needed.
fn shared_programs() -> String {
    let task = format!(
        "{}{}{}{}{}{}",
        refresh_task("NEW.task_id"),
        refresh_context("NEW.task_id"),
        // changes() observes the immediately preceding context statement in
        // this trigger frame. Unchanged inputs retain valid numeric state.
        "INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE changes()<>0;",
        apply("card", "NEW.task_id"),
        apply("metadata", "NEW.task_id"),
        apply("choices", "NEW.task_id")
    );
    let delete="INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries
            WHERE task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.task_id);
        DELETE FROM tracker_task_summaries WHERE task_id=NEW.task_id;
        DELETE FROM tracker_task_summary_context WHERE task_id=NEW.task_id;
        DELETE FROM tracker_task_summary_facts WHERE task_id=NEW.task_id;
        DELETE FROM tracker_task_filter_choices WHERE task_id=NEW.task_id;".to_string();
    let definitions = [
        ("task", task),
        ("delete", delete),
        ("metadata", refresh_metadata("NEW.task_id")),
        ("choices", choices::refresh("NEW.task_id")),
        ("dependencies", refresh_dependencies("task_id=NEW.task_id")),
        ("identity", refresh_identities("a.id=NEW.task_id")),
    ];
    definitions.into_iter().map(|(name,body)|format!("CREATE VIEW IF NOT EXISTS tracker_summary_apply_{name} AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_{name}_run INSTEAD OF INSERT ON tracker_summary_apply_{name} BEGIN {body} END;"))
        .collect::<Vec<_>>().join("\n")
}

pub(super) fn refresh_metadata(id: &str) -> String {
    format!("UPDATE tracker_task_summaries SET (record,assignment_target)=(SELECT
        json_set(json_patch(json_remove(tracker_task_summaries.record,'$.review_summary','$.deferred_assignment'),m.patch),
            '$._summary_invalid_metadata_refs',json(CASE WHEN m.invalid_refs THEN 'true' ELSE 'false' END)),m.assignment_target
        FROM tracker_summary_metadata m WHERE m.task_id=tracker_task_summaries.task_id) WHERE task_id={id};")
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

pub(super) fn triggers() -> String {
    let mut sql = helpers();
    sql.push_str(&shared_programs());
    sql.push_str(&task_triggers());
    sql.push_str(&activity::triggers());
    for table in ["tracker_comments", "tracker_events"] {
        sql.push_str(&history_triggers(table));
    }
    for table in ["reviews", "tracker_delivery_intents"] {
        sql.push_str(&metadata_triggers(table));
    }
    sql.push_str(&edge_triggers());
    sql.push_str(&identity_triggers());

    sql
}

fn task_triggers() -> String {
    let task = apply("task", "NEW.id");
    let delete = apply("delete", "OLD.id");
    let dependencies="INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id=NEW.id OR task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.id);";
    format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_task_INSERT AFTER INSERT ON tracker_tasks BEGIN {task}{dependencies} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_UPDATE AFTER UPDATE ON tracker_tasks
        WHEN OLD.id IS NEW.id AND (OLD.record IS NOT NEW.record OR OLD.project_key IS NOT NEW.project_key OR OLD.number IS NOT NEW.number
            OR OLD.state IS NOT NEW.state OR OLD.status IS NOT NEW.status OR OLD.created_at IS NOT NEW.created_at OR OLD.updated_at IS NOT NEW.updated_at)
        BEGIN {task} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_key AFTER UPDATE OF id ON tracker_tasks WHEN OLD.id IS NOT NEW.id
        BEGIN {delete}{task}{dependencies} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_dependencies AFTER UPDATE OF state,status ON tracker_tasks
            WHEN OLD.id IS NEW.id AND (OLD.state IS NOT NEW.state OR OLD.status IS NOT NEW.status) BEGIN {dependencies} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_delete AFTER DELETE ON tracker_tasks BEGIN {delete} END;")
}

fn history_triggers(table: &str) -> String {
    let kind = if table == "tracker_comments" {
        "comment"
    } else {
        "event"
    };
    let insert = format!("{};", fact_insert(table, "id=NEW.fact_id"));
    let (at, paths) = if table == "tracker_comments" {
        (
            "created_at",
            "'$.author.kind','$.mentions_user','$.notifies_user'",
        )
    } else {
        ("at","'$.kind','$.actor.kind','$.payload.by','$.payload.waiting_on','$.payload.to','$.mentions_user','$.notifies_user'")
    };
    // Capture replacement state before mutating facts. The source-owned routine
    // maintains facts and counters immediately without derived-table triggers,
    // whose DELETE/INSERT conflict policies otherwise duplicate compiled work.
    format!("CREATE VIEW IF NOT EXISTS tracker_summary_apply_{kind} AS SELECT
            CAST(NULL AS TEXT) task_id,CAST(NULL AS TEXT) fact_id,CAST(NULL AS TEXT) old_task_id,
            CAST(NULL AS TEXT) old_fact_id,CAST(NULL AS INTEGER) replace_existing WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_{kind}_run INSTEAD OF INSERT ON tracker_summary_apply_{kind} BEGIN
            DELETE FROM tracker_task_summary_facts WHERE (task_id=NEW.task_id AND id=NEW.fact_id)
                OR (task_id=NEW.old_task_id AND id=NEW.old_fact_id);
            {insert}
            INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE NEW.replace_existing=1 AND NEW.task_id IS NOT NULL
                UNION SELECT NEW.old_task_id WHERE NEW.old_task_id IS NOT NULL;
            INSERT INTO tracker_summary_apply_append SELECT NEW.task_id,NEW.fact_id WHERE NEW.replace_existing=0; END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_INSERT AFTER INSERT ON {table} BEGIN
            INSERT INTO tracker_summary_apply_{kind} SELECT NEW.task_id,NEW.id,NULL,NULL,
                EXISTS(SELECT 1 FROM tracker_task_summary_facts WHERE task_id=NEW.task_id AND id=NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_UPDATE AFTER UPDATE ON {table}
        WHEN OLD.task_id IS NOT NEW.task_id OR OLD.id IS NOT NEW.id OR OLD.{at} IS NOT NEW.{at}
            OR json_extract(OLD.record,{paths}) IS NOT json_extract(NEW.record,{paths})
        BEGIN INSERT INTO tracker_summary_apply_{kind} VALUES(NEW.task_id,NEW.id,OLD.task_id,OLD.id,1); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_DELETE AFTER DELETE ON {table} BEGIN
            INSERT INTO tracker_summary_apply_{kind} VALUES(NULL,NULL,OLD.task_id,OLD.id,1); END;")
}

fn metadata_triggers(table: &str) -> String {
    let mut sql = String::new();
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let select = match operation {
            "INSERT" => "SELECT NEW.task_id",
            "DELETE" => "SELECT OLD.task_id",
            _ => "SELECT OLD.task_id UNION SELECT NEW.task_id",
        };
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_{table}_{operation} AFTER {operation} ON {table} BEGIN
            INSERT INTO tracker_summary_apply_metadata {select}; END;"));
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
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_edges_{operation} AFTER {operation} ON tracker_task_dependencies BEGIN
            INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE {predicate}; END;"));
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
    format!("UPDATE tracker_task_summaries SET record=json_set(record,'$.dependencies',
        json((SELECT record FROM tracker_summary_dependencies d WHERE d.task_id=tracker_task_summaries.task_id))) WHERE {predicate};")
}

fn identity_rows() -> &'static str {
    "SELECT a.rowid source_rowid,a.id,a.owner_id,json_object('agent_id',a.id,'name',substr(json_extract(a.record,'$.name'),1,128),
        'project_role',substr(json_extract(a.record,'$.project_role'),1,64),'ordinal',a.ordinal,
        'provider',substr(COALESCE(json_extract(a.record,'$.choice.provider'),'claude'),1,64),
        'workspace_id',NULL,'workspace_name',CASE WHEN json_extract(a.record,'$.project_role') IS NULL
            THEN substr(json_extract(r.record,'$.worktree_name'),1,128) END,'available',json('false')) record
        FROM agents a LEFT JOIN implementations r ON r.id=a.owner_id WHERE length(CAST(a.id AS BLOB))<=128"
}

/// Owner metadata is parsed on writes only. Reads seek a bounded identity row.
pub(super) fn refresh_identities(predicate: &str) -> String {
    format!("INSERT INTO tracker_task_summary_identities(agent_id,record) SELECT a.id,a.record FROM tracker_summary_identity a WHERE {predicate}
        ON CONFLICT(agent_id) DO UPDATE SET record=excluded.record WHERE record IS NOT excluded.record;",predicate=predicate.replace("a.rowid","a.source_rowid"))
}

fn identity_triggers() -> String {
    let insert = apply("identity", "NEW.id");
    let mut sql=format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_INSERT AFTER INSERT ON agents BEGIN {insert} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_UPDATE AFTER UPDATE ON agents
        WHEN OLD.id IS NOT NEW.id OR OLD.owner_id IS NOT NEW.owner_id OR OLD.ordinal IS NOT NEW.ordinal
            OR json_extract(OLD.record,'$.name','$.project_role','$.choice.provider') IS NOT json_extract(NEW.record,'$.name','$.project_role','$.choice.provider')
        BEGIN DELETE FROM tracker_task_summary_identities WHERE agent_id=OLD.id AND OLD.id<>NEW.id; {insert} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_delete AFTER DELETE ON agents BEGIN
            DELETE FROM tracker_task_summary_identities WHERE agent_id=OLD.id; END;");
    for owner in ["implementations", "tasks"] {
        sql.push_str(&owner_identity_triggers(owner));
    }
    sql
}

fn owner_identity_triggers(owner: &str) -> String {
    let mut sql = String::new();
    for operation in ["INSERT", "UPDATE", "DELETE"] {
        let id = if operation == "DELETE" {
            "OLD.id"
        } else {
            "NEW.id"
        };
        let guard = if operation == "UPDATE" {
            "WHEN OLD.id IS NOT NEW.id OR json_extract(OLD.record,'$.worktree_name') IS NOT json_extract(NEW.record,'$.worktree_name')"
        } else {
            ""
        };
        let predicate = if operation == "UPDATE" {
            "a.owner_id IN (OLD.id,NEW.id)".to_string()
        } else {
            format!("a.owner_id={id}")
        };
        let predicate = predicate.replace("a.owner_id", "owner_id");
        sql.push_str(&format!("CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_{owner}_{operation} AFTER {operation} ON {owner} {guard} BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE {predicate}; END;"));
    }
    sql
}

#[cfg(test)]
#[path = "sql/tests.rs"]
mod tests;
