//! Typed retained activity state; keyed views own the full and append derivations.

pub(super) fn context() -> &'static str {
    r#"json_object('watched',COALESCE(json_extract(t.record,'$.watched'),0),
        'read_ulid',substr(substr(json_extract(t.record,'$.read_through'),instr(json_extract(t.record,'$.read_through'),'-')+1),1,128),
        'watch_ulid',substr(substr(json_extract(t.record,'$.watch_started_after'),instr(json_extract(t.record,'$.watch_started_after'),'-')+1),1,128),
        'session_start',json_extract(t.record,'$.activity_session.session_started_ms'),
        'session_last',json_extract(t.record,'$.activity_session.last_activity_ms'),
        'done_at',substr(json_extract(t.record,'$.done_at'),1,64),'status',substr(t.status,1,128),
        'created_at',substr(t.created_at,1,64),
        'invalid_assignee',COALESCE(length(CAST(json_extract(t.record,'$.assignee.agent_id') AS BLOB))>128,0))"#
}

const RESULT_COLUMNS: &str = "unread_count,unread_agent_comments,unread_mentions,session_started_ms,last_activity_ms,done_at";

pub(super) fn refresh(id: &str) -> String {
    format!(
        "UPDATE tracker_task_summary_context SET ({RESULT_COLUMNS})=(SELECT {RESULT_COLUMNS}
        FROM tracker_summary_activity WHERE task_id={id}) WHERE task_id={id};"
    )
}

pub(super) fn views() -> String {
    format!(
        "CREATE VIEW IF NOT EXISTS tracker_summary_activity AS {full};
        CREATE VIEW IF NOT EXISTS tracker_summary_append AS {append};",
        full = full(),
        append = append()
    )
}

fn done_at() -> &'static str {
    "CASE WHEN c.status='done' THEN COALESCE(c.input_done_at,
        (SELECT at FROM tracker_task_summary_facts WHERE task_id=c.task_id AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1),c.created_at) END"
}

fn full() -> String {
    format!(
        r#"SELECT c.task_id,
        CASE WHEN c.watched=1 THEN (SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=c.task_id AND f.unread=1
            AND f.ulid>COALESCE(c.read_ulid,'') AND f.ulid>COALESCE(c.watch_ulid,
                (SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=c.task_id AND watched=1),'')) ELSE 0 END unread_count,
        CASE WHEN c.watched=1 THEN (SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=c.task_id AND f.agent_comment=1
            AND f.ulid>COALESCE(c.read_ulid,'')) ELSE 0 END unread_agent_comments,
        CASE WHEN c.watched=1 THEN (SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=c.task_id AND f.mentions=1
            AND f.ulid>COALESCE(c.read_ulid,'')) ELSE 0 END unread_mentions,
        CASE WHEN c.watched=1 THEN (SELECT COALESCE(MAX(CASE WHEN prev IS NOT NULL AND ts-prev>=28800000 THEN ts END),
            c.session_start,MIN(ts)) FROM (SELECT ts,LAG(ts) OVER (ORDER BY ts) prev FROM (
                SELECT c.session_last ts UNION SELECT ts FROM tracker_task_summary_facts WHERE task_id=c.task_id AND activity=1
                    AND ts>COALESCE(c.session_last,-9223372036854775808)) WHERE ts IS NOT NULL)) END session_started_ms,
        CASE WHEN c.watched=1 THEN (SELECT MAX(ts) FROM (SELECT c.session_last ts UNION ALL
            SELECT ts FROM tracker_task_summary_facts WHERE task_id=c.task_id AND activity=1
                AND ts>COALESCE(c.session_last,-9223372036854775808))) END last_activity_ms,
        {done} done_at FROM tracker_task_summary_context c"#,
        done = done_at()
    )
}

fn append() -> String {
    "SELECT task_id,id fact_id,ulid,ts,unread,activity,agent_comment,mentions FROM tracker_task_summary_facts WHERE watched=0".to_string()
}

fn append_update() -> String {
    // Read the retained values from the UPDATE target itself. A derived view
    // joining the same context table would perform another lookup and expand
    // its arithmetic again during trigger compilation.
    format!(r#"UPDATE tracker_task_summary_context SET
        unread_count=c.unread_count+(c.watched=1 AND f.unread=1 AND f.ulid>COALESCE(c.read_ulid,'') AND
            f.ulid>COALESCE(c.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=c.task_id AND watched=1),'')),
        unread_agent_comments=c.unread_agent_comments+(c.watched=1 AND f.agent_comment=1 AND f.ulid>COALESCE(c.read_ulid,'')),
        unread_mentions=c.unread_mentions+(c.watched=1 AND f.mentions=1 AND f.ulid>COALESCE(c.read_ulid,'')),
        session_started_ms=CASE WHEN c.watched=1 THEN CASE WHEN f.activity=1 AND f.ts IS NOT NULL THEN
            CASE WHEN c.session_started_ms IS NULL OR c.last_activity_ms IS NULL OR f.ts-c.last_activity_ms>=28800000 THEN f.ts ELSE c.session_started_ms END
            ELSE c.session_started_ms END END,
        last_activity_ms=CASE WHEN c.watched=1 THEN CASE WHEN f.activity=1 AND f.ts IS NOT NULL THEN f.ts ELSE c.last_activity_ms END END,
        done_at={done} FROM tracker_summary_append f WHERE c.task_id=NEW.task_id AND f.task_id=NEW.task_id AND f.fact_id=NEW.fact_id
            AND (c.watched<>1 OR f.activity=0 OR f.ts IS NULL OR c.last_activity_ms IS NULL OR f.ts>=c.last_activity_ms);"#,
        done=done_at()).replace("c.","tracker_task_summary_context.")
}

pub(super) fn triggers() -> String {
    format!("CREATE VIEW IF NOT EXISTS tracker_summary_apply_activity AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE VIEW IF NOT EXISTS tracker_summary_apply_append AS SELECT CAST(NULL AS TEXT) task_id,CAST(NULL AS TEXT) fact_id WHERE 0;
        CREATE VIEW IF NOT EXISTS tracker_summary_apply_card AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_activity_run INSTEAD OF INSERT ON tracker_summary_apply_activity BEGIN
            {refresh} INSERT INTO tracker_summary_apply_card VALUES(NEW.task_id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_card_run INSTEAD OF INSERT ON tracker_summary_apply_card BEGIN {patch} END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_append_run INSTEAD OF INSERT ON tracker_summary_apply_append BEGIN
            {append}
            INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE changes()=0;
            INSERT INTO tracker_summary_apply_card VALUES(NEW.task_id); END;
",refresh=refresh("NEW.task_id"),patch=patch_card("NEW.task_id"),append=append_update())
}

pub(super) fn patch_card(id: &str) -> String {
    format!("UPDATE tracker_task_summaries SET record=(SELECT CASE WHEN c.watched=1 THEN json_set(tracker_task_summaries.record,
        '$.unread_count',c.unread_count,'$.unread_agent_comments',c.unread_agent_comments,'$.unread_mentions',c.unread_mentions,
        '$.session_started_ms',c.session_started_ms,'$.last_activity_ms',c.last_activity_ms,'$.done_at',c.done_at)
        ELSE json_set(json_remove(tracker_task_summaries.record,'$.unread_count','$.unread_agent_comments','$.unread_mentions','$.session_started_ms','$.last_activity_ms'),
            '$.done_at',c.done_at) END FROM tracker_task_summary_context c WHERE c.task_id={id}) WHERE task_id={id};")
}
