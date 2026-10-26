//! History-derived patches use only bounded context and compact fact columns.

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

pub(super) fn refresh(id: &str) -> String {
    refresh_when(id, "1")
}

fn refresh_when(id: &str, predicate: &str) -> String {
    format!("UPDATE tracker_task_summaries SET record=(SELECT json_set(json_patch(t.card,{stats}),'$.done_at',
        CASE WHEN t.status='done' THEN COALESCE(t.done_at,
            (SELECT at FROM tracker_task_summary_facts WHERE task_id=t.id AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1),t.created_at) END)
        FROM (WITH ctx AS MATERIALIZED (SELECT task_id AS id,record AS card,
            json_extract(context,'$.watched') AS watched,json_extract(context,'$.read_ulid') AS read_ulid,
            json_extract(context,'$.watch_ulid') AS watch_ulid,
            json_extract(context,'$.session_start') AS session_start,json_extract(context,'$.session_last') AS session_last,
            json_extract(context,'$.done_at') AS done_at,json_extract(context,'$.status') AS status,
            json_extract(context,'$.created_at') AS created_at FROM tracker_task_summaries WHERE task_id={id}) SELECT * FROM ctx) t)
        WHERE task_id={id} AND ({predicate});",stats=stats_patch())
}

/// A chronological append needs one new fact and the retained card, never a
/// replay of the history prefix. Watch-floor changes and late activity retain
/// the full compact-fact derivation used for edits and removals.
pub(super) fn inserted(id: &str, fact: &str) -> String {
    let safe=format!("EXISTS(SELECT 1 FROM tracker_task_summary_facts f WHERE f.task_id={id} AND f.id={fact} AND f.watched=0
        AND (COALESCE(json_extract(context,'$.watched'),0)<>1 OR f.activity=0 OR f.ts IS NULL
            OR json_extract(record,'$.last_activity_ms') IS NULL OR f.ts>=json_extract(record,'$.last_activity_ms')))");
    format!("UPDATE tracker_task_summaries SET record=(SELECT json_set(json_patch(t.card,{stats}),'$.done_at',
        CASE WHEN t.status='done' THEN COALESCE(t.done_at,
            (SELECT at FROM tracker_task_summary_facts WHERE task_id=t.id AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1),t.created_at) END)
        FROM (WITH ctx AS MATERIALIZED (SELECT s.task_id AS id,s.record AS card,
            json_extract(s.context,'$.watched') AS watched,json_extract(s.context,'$.read_ulid') AS read_ulid,
            json_extract(s.context,'$.watch_ulid') AS watch_ulid,json_extract(s.context,'$.done_at') AS done_at,
            json_extract(s.context,'$.status') AS status,json_extract(s.context,'$.created_at') AS created_at,
            f.ulid,f.ts,f.unread,f.agent_comment,f.mentions,f.activity
            FROM tracker_task_summaries s JOIN tracker_task_summary_facts f ON f.task_id=s.task_id
            WHERE s.task_id={id} AND f.id={fact}) SELECT * FROM ctx) t)
        WHERE task_id={id} AND ({safe});
        {fallback}",stats=increment_patch(),fallback=refresh_when(id,&format!("NOT ({safe})")))
}

fn increment_patch() -> &'static str {
    r#"CASE WHEN t.watched=1 THEN json_patch(json_object(
        'unread_agent_comments',COALESCE(json_extract(t.card,'$.unread_agent_comments'),0)+(t.agent_comment=1 AND t.ulid>COALESCE(t.read_ulid,'')),
        'unread_mentions',COALESCE(json_extract(t.card,'$.unread_mentions'),0)+(t.mentions=1 AND t.ulid>COALESCE(t.read_ulid,'')),
        'unread_count',COALESCE(json_extract(t.card,'$.unread_count'),0)+(t.unread=1 AND t.ulid>COALESCE(t.read_ulid,'') AND
            t.ulid>COALESCE(t.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=t.id AND watched=1),''))),
        CASE WHEN t.activity=1 AND t.ts IS NOT NULL THEN json_object(
            'session_started_ms',CASE WHEN json_extract(t.card,'$.session_started_ms') IS NULL
                OR json_extract(t.card,'$.last_activity_ms') IS NULL
                OR t.ts-json_extract(t.card,'$.last_activity_ms')>=28800000 THEN t.ts
                ELSE json_extract(t.card,'$.session_started_ms') END,
            'last_activity_ms',t.ts) ELSE json('{}') END) ELSE json('{}') END"#
}

fn stats_patch() -> &'static str {
    r#"CASE WHEN t.watched=1 THEN json_object(
        'unread_agent_comments',(SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=t.id AND f.agent_comment=1
            AND f.ulid>COALESCE(t.read_ulid,'')),
        'unread_mentions',(SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=t.id AND f.mentions=1
            AND f.ulid>COALESCE(t.read_ulid,'')),
        'unread_count',(SELECT COUNT(*) FROM tracker_task_summary_facts f WHERE f.task_id=t.id AND f.unread=1
            AND f.ulid>COALESCE(t.read_ulid,'')
            AND f.ulid>COALESCE(t.watch_ulid,
                (SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=t.id AND watched=1),'')),
        'session_started_ms',(SELECT COALESCE(MAX(CASE WHEN prev IS NOT NULL AND ts-prev>=28800000 THEN ts END),
            t.session_start,MIN(ts)) FROM (
            SELECT ts,LAG(ts) OVER (ORDER BY ts) prev FROM (
                SELECT t.session_last ts
                UNION SELECT ts FROM tracker_task_summary_facts WHERE task_id=t.id AND activity=1
                    AND ts>COALESCE(t.session_last,-9223372036854775808)
            ) WHERE ts IS NOT NULL)),
        'last_activity_ms',(SELECT MAX(ts) FROM (
            SELECT t.session_last ts
            UNION ALL SELECT ts FROM tracker_task_summary_facts WHERE task_id=t.id AND activity=1
                AND ts>COALESCE(t.session_last,-9223372036854775808)))
        ) ELSE json('{}') END"#
}
