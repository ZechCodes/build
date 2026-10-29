//! Retained activity state with keyed full and incremental write derivations.

pub(in crate::store::task_summaries) fn context() -> String {
    format!(
        r#"json_object('watched',COALESCE(json_extract(t.record,'$.watched'),0),
        'read_ulid',{read},
        'watch_ulid',{watch},
        'session_start',json_extract(t.record,'$.activity_session.session_started_ms'),
        'session_last',json_extract(t.record,'$.activity_session.last_activity_ms'),
        'done_at',substr(json_extract(t.record,'$.done_at'),1,64),'status',substr(t.status,1,128),
        'created_at',substr(t.created_at,1,64),
        'invalid_assignee',COALESCE(length(CAST(json_extract(t.record,'$.assignee.agent_id') AS BLOB))>128,0))"#,
        read = bounded_marker("json_extract(t.record,'$.read_through')"),
        watch = bounded_marker("json_extract(t.record,'$.watch_started_after')")
    )
}

const RESULT_COLUMNS: &str = "unread_count,unread_agent_comments,unread_mentions,session_started_ms,last_activity_ms,done_at";

pub(in crate::store::task_summaries) fn refresh(id: &str) -> String {
    format!(
        "UPDATE tracker_task_summary_context SET ({RESULT_COLUMNS})=(SELECT {RESULT_COLUMNS}
        FROM ({full}) WHERE task_id={id}) WHERE task_id={id};",
        full = full()
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
            AND (c.read_ulid IS NULL OR f.ulid>c.read_ulid) AND (COALESCE(c.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=c.task_id AND watched=1)) IS NULL OR f.ulid>COALESCE(c.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=c.task_id AND watched=1)))) ELSE 0 END unread_count,
        CASE WHEN c.watched=1 THEN (SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=c.task_id AND f.agent_comment=1
            AND (c.read_ulid IS NULL OR f.ulid>c.read_ulid)) ELSE 0 END unread_agent_comments,
        CASE WHEN c.watched=1 THEN (SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=c.task_id AND f.mentions=1
            AND (c.read_ulid IS NULL OR f.ulid>c.read_ulid)) ELSE 0 END unread_mentions,
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

pub(in crate::store::task_summaries) fn append_update() -> String {
    // Read retained values from the UPDATE target and seek one fact by its
    // primary key. Older or watch-changing entries use the full derivation.
    format!(r#"UPDATE tracker_task_summary_context SET
        unread_count=c.unread_count+(c.watched=1 AND f.unread=1 AND (c.read_ulid IS NULL OR f.ulid>c.read_ulid) AND
            (COALESCE(c.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=c.task_id AND watched=1)) IS NULL OR f.ulid>COALESCE(c.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=c.task_id AND watched=1)))),
        unread_agent_comments=c.unread_agent_comments+(c.watched=1 AND f.agent_comment=1 AND (c.read_ulid IS NULL OR f.ulid>c.read_ulid)),
        unread_mentions=c.unread_mentions+(c.watched=1 AND f.mentions=1 AND (c.read_ulid IS NULL OR f.ulid>c.read_ulid)),
        session_started_ms=CASE WHEN c.watched=1 THEN CASE WHEN f.activity=1 AND f.ts IS NOT NULL THEN
            CASE WHEN c.session_started_ms IS NULL OR c.last_activity_ms IS NULL OR f.ts-c.last_activity_ms>=28800000 THEN f.ts ELSE c.session_started_ms END
            ELSE c.session_started_ms END END,
        last_activity_ms=CASE WHEN c.watched=1 THEN CASE WHEN f.activity=1 AND f.ts IS NOT NULL THEN f.ts ELSE c.last_activity_ms END END,
        done_at={done} FROM tracker_task_summary_facts f WHERE c.task_id=?1 AND f.task_id=?1 AND f.id=?2 AND f.watched=0
            AND (c.watched<>1 OR f.activity=0 OR f.ts IS NULL OR c.last_activity_ms IS NULL OR f.ts>=c.last_activity_ms);"#,
        done=done_at()).replace("c.","tracker_task_summary_context.")
}

pub(in crate::store::task_summaries) fn patch_card(id: &str) -> String {
    format!("UPDATE tracker_task_summaries SET record=(SELECT CASE WHEN c.watched=1 THEN json_set(tracker_task_summaries.record,
        '$.unread_count',c.unread_count,'$.unread_agent_comments',c.unread_agent_comments,'$.unread_mentions',c.unread_mentions,
        '$.session_started_ms',c.session_started_ms,'$.last_activity_ms',c.last_activity_ms,'$.done_at',c.done_at)
        ELSE json_set(json_remove(tracker_task_summaries.record,'$.unread_count','$.unread_agent_comments','$.unread_mentions','$.session_started_ms','$.last_activity_ms'),
            '$.done_at',c.done_at) END FROM tracker_task_summary_context c WHERE c.task_id={id}) WHERE task_id={id};")
}

fn bounded_marker(value: &str) -> String {
    format!(
        "CASE WHEN length(CAST(({value}) AS BLOB))>128 THEN '!unsupported-marker' ELSE {} END",
        super::opaque_suffix(value)
    )
}
