//! Synchronize only changed memberships from the bounded card projection.

pub(super) fn refresh(id: &str) -> String {
    let desired = desired(id);
    format!("DELETE FROM tracker_task_filter_choices AS old WHERE task_id={id} AND NOT EXISTS(
        SELECT 1 FROM ({desired}) d WHERE d.project_key=old.project_key AND d.dimension=old.dimension
            AND d.choice_key=old.choice_key AND d.value=old.value);
        INSERT INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value)
        SELECT project_key,dimension,choice_key,task_id,value FROM ({desired}) d WHERE NOT EXISTS(
            SELECT 1 FROM tracker_task_filter_choices old WHERE old.project_key=d.project_key AND old.dimension=d.dimension
                AND old.choice_key=d.choice_key AND old.task_id=d.task_id AND old.value=d.value);")
}

fn desired(id: &str) -> String {
    let raw = format!(
        r#"WITH card AS MATERIALIZED (SELECT project_key,task_id,jsonb(record) record,jsonb(context) context
            FROM tracker_task_summaries WHERE task_id={id}),
        assigned AS MATERIALIZED (SELECT project_key,task_id,CASE
            WHEN json_extract(context,'$.invalid_assignee') THEN 'null'
            WHEN json_extract(record,'$.assignee') IS NULL THEN json_quote('none')
            WHEN json_extract(record,'$.assignee.kind')='agent' THEN json_object('agent_id',json_extract(record,'$.assignee.agent_id'),'kind','agent')
            ELSE json(json_extract(record,'$.assignee')) END value FROM card)
        SELECT c.project_key,'label' dimension,lower(j.value) choice_key,c.task_id,json_quote(j.value) value
            FROM card c,json_each(c.record,'$.labels') j WHERE NOT json_extract(c.record,'$._summary_invalid_labels')
        UNION SELECT project_key,'label','!invalid:'||task_id,task_id,'null' FROM card WHERE json_extract(record,'$._summary_invalid_labels')
        UNION SELECT project_key,'status',json_extract(record,'$.status'),task_id,json_quote(json_extract(record,'$.status')) FROM card
        UNION SELECT project_key,'state',json_extract(record,'$.state'),task_id,json_quote(json_extract(record,'$.state')) FROM card
        UNION SELECT project_key,'assignee',CASE WHEN value='null' THEN '!invalid:'||task_id ELSE value END,task_id,value FROM assigned"#
    );
    format!("WITH raw AS MATERIALIZED ({raw}) SELECT project_key,dimension,choice_key,task_id,MIN(value) AS value
        FROM raw GROUP BY project_key,dimension,choice_key,task_id")
}
