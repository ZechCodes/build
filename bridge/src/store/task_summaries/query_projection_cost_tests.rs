//! Physical work proof runs the exact INSERT statement used by refresh().
use super::*;
use rusqlite::StatementStatus;

fn execute_refresh(conn: &Connection, id: &str, floor: Option<&str>) -> (u64, bool) {
    conn.execute(
        "DELETE FROM tracker_task_query_facts WHERE task_id=?1",
        [id],
    )
    .unwrap();
    let mut statement = conn.prepare_cached(QUERY_FACT_REFRESH).unwrap();
    statement.reset_status(StatementStatus::VmStep);
    statement
        .execute(params![
            id,
            Option::<String>::None,
            1_i64,
            1_i64,
            Option::<i64>::None,
            false,
            false,
            false,
            floor,
            true,
            false,
            Option::<String>::None,
            Option::<String>::None,
            false,
            false,
            false
        ])
        .unwrap();
    let steps = statement.get_status(StatementStatus::VmStep) as u64;
    let attention = conn
        .query_row(
            "SELECT attention_legacy FROM tracker_task_query_facts WHERE task_id=?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    (steps, attention)
}
fn fixture() -> (tempfile::TempDir, Store, String) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(dir.path()).unwrap();
    let mut draft = crate::tracker::Task::drafted(
        "/meaningful-cost",
        "cost",
        crate::tracker::Actor::User,
        "2026-10-01T00:00:00Z",
    );
    draft.watched = true;
    let task = store.create_tracker_task(draft, &[]).unwrap();
    // Explicit derived cost fixture: force the eligible inbox branch without
    // running unrelated unread-history reduction in the measured statement.
    store.connection().execute("UPDATE tracker_task_summaries SET record=json_set(record,'$.unread_count',1,'$.unread_agent_comments',1) WHERE task_id=?1",[&task.id]).unwrap();
    (dir, store, task.id)
}
fn populate(conn: &Connection, id: &str, n: usize) {
    conn.execute(
        "DELETE FROM tracker_task_summary_facts WHERE task_id=?1",
        [id],
    )
    .unwrap();
    conn.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<?2) INSERT INTO tracker_task_summary_facts(task_id,id,ulid,at,ts,unread,activity,meaningful,watched,done_move,agent_comment,mentions) SELECT ?1,printf('tc-%06d',x),printf('%06d',x),'2026-10-01T00:00:00Z',0,0,0,1,0,0,0,0 FROM n",params![id,n as i64]).unwrap();
}
#[test]
fn actual_query_refresh_meaningful_floor_is_an_index_range_with_bounded_vm_steps() {
    let (_dir, store, id) = fixture();
    let conn = store.connection();
    let mut observed = Vec::new();
    for n in [10, 10_000, 50_000] {
        populate(&conn, &id, n);
        for floor in [None, Some(""), Some("009999"), Some("999999")] {
            let (steps, attention) = execute_refresh(&conn, &id, floor);
            eprintln!("actual_query_refresh history={n} dismissed={floor:?} vm_steps={steps} attention={attention}");
            assert_eq!(
                attention,
                floor.is_none_or(|floor| format!("{n:06}").as_str() > floor),
                "eligible legacy branch must execute for {n} {floor:?}"
            );
            observed.push((n, floor, steps));
        }
    }
    let plan = conn
        .prepare(&format!("EXPLAIN QUERY PLAN {QUERY_FACT_REFRESH}"))
        .unwrap()
        .query_map(
            params![
                id,
                Option::<String>::None,
                1_i64,
                1_i64,
                Option::<i64>::None,
                false,
                false,
                false,
                Some("999999"),
                true,
                false,
                Option::<String>::None,
                Option::<String>::None,
                false,
                false,
                false
            ],
            |r| r.get::<_, String>(3),
        )
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(";");
    eprintln!("actual_query_refresh_plan={plan}");
    assert!(
        observed.iter().all(|(_, _, steps)| *steps < 1000),
        "actual production INSERT exceeded bounded work: {observed:?}"
    );
    assert!(
        plan.contains("tracker_summary_meaningful") && plan.contains("ulid>?)"),
        "nonnull dismissal must seek the ULID range: {plan}"
    );
}
#[test]
fn actual_query_refresh_distinguishes_null_floor_from_explicit_empty_floor() {
    let (_dir, store, id) = fixture();
    let conn = store.connection();
    conn.execute("INSERT INTO tracker_task_summary_facts(task_id,id,ulid,at,ts,unread,activity,meaningful,watched,done_move,agent_comment,mentions) VALUES(?1,'tc-','','2026-10-01T00:00:00Z',0,0,0,1,0,0,0,0)",[&id]).unwrap();
    assert!(execute_refresh(&conn, &id, None).1);
    assert!(!execute_refresh(&conn, &id, Some("")).1);
}
