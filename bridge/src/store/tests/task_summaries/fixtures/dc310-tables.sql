
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
