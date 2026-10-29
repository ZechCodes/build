CREATE VIEW IF NOT EXISTS tracker_summary_comment_facts AS SELECT rowid source_rowid,task_id,id,CASE WHEN instr(id,'-')>0 THEN substr(id,instr(id,'-')+1) ELSE id END ulid,
            created_at at,CASE WHEN instr(created_at,'.')=0 THEN unixepoch(created_at)*1000 ELSE
        (unixepoch(substr(created_at,1,19)||(CASE WHEN substr(created_at,-1)='Z' THEN 'Z' ELSE substr(created_at,-6) END)))*1000+CAST(substr((substr(created_at,instr(created_at,'.')+1,length(created_at)-instr(created_at,'.')-CASE WHEN substr(created_at,-1)='Z' THEN 1 ELSE 6 END))||'000',1,3) AS INTEGER)+
        CASE WHEN (unixepoch(substr(created_at,1,19)||(CASE WHEN substr(created_at,-1)='Z' THEN 'Z' ELSE substr(created_at,-6) END)))<0 AND substr((substr(created_at,instr(created_at,'.')+1,length(created_at)-instr(created_at,'.')-CASE WHEN substr(created_at,-1)='Z' THEN 1 ELSE 6 END)),4) GLOB '*[1-9]*' THEN 1 ELSE 0 END END ts,
            COALESCE((json_extract(record,'$.author.kind') <> 'user'),0) unread,COALESCE((1),0) activity,COALESCE((0),0) watched,COALESCE((0),0) done_move,COALESCE((json_extract(record,'$.author.kind')='agent'),0) agent_comment,COALESCE((json_extract(record,'$.author.kind')='agent' AND (json_extract(record,'$.mentions_user')=1 OR json_extract(record,'$.notifies_user')=1)),0) mentions
        FROM tracker_comments;CREATE VIEW IF NOT EXISTS tracker_summary_event_facts AS SELECT rowid source_rowid,task_id,id,CASE WHEN instr(id,'-')>0 THEN substr(id,instr(id,'-')+1) ELSE id END ulid,
            at at,CASE WHEN instr(at,'.')=0 THEN unixepoch(at)*1000 ELSE
        (unixepoch(substr(at,1,19)||(CASE WHEN substr(at,-1)='Z' THEN 'Z' ELSE substr(at,-6) END)))*1000+CAST(substr((substr(at,instr(at,'.')+1,length(at)-instr(at,'.')-CASE WHEN substr(at,-1)='Z' THEN 1 ELSE 6 END))||'000',1,3) AS INTEGER)+
        CASE WHEN (unixepoch(substr(at,1,19)||(CASE WHEN substr(at,-1)='Z' THEN 'Z' ELSE substr(at,-6) END)))<0 AND substr((substr(at,instr(at,'.')+1,length(at)-instr(at,'.')-CASE WHEN substr(at,-1)='Z' THEN 1 ELSE 6 END)),4) GLOB '*[1-9]*' THEN 1 ELSE 0 END END ts,
            COALESCE((json_extract(record,'$.actor.kind') <> 'user' AND NOT (json_extract(record,'$.kind')='moved' AND COALESCE(json_extract(record,'$.payload.by'),'') IN ('dispatch','report')) AND
            (json_extract(record,'$.kind') IN ('assigned','unassigned','moved','closed','reopened','review_completed') OR
             (json_extract(record,'$.kind')='created' AND json_extract(record,'$.mentions_user')=1 AND json_extract(record,'$.actor.kind')='agent'))),0) unread,COALESCE((NOT (json_extract(record,'$.kind') IN ('blocked','unblocked','dispatch_deferred','dispatch_failed','dispatched',
            'workspace_idle','workspace_pruned','workspace_reclaimed','branch_deleted','branch_kept') OR
            (json_extract(record,'$.kind') IN ('linked','unlinked') AND json_type(record,'$.payload.waiting_on') IS NOT NULL) OR
            (json_extract(record,'$.kind')='tracked' AND COALESCE(json_extract(record,'$.payload.by'),'')='assignment') OR (json_extract(record,'$.kind')='moved' AND COALESCE(json_extract(record,'$.payload.by'),'') IN ('dispatch','report')) OR
            (json_extract(record,'$.kind')='untracked' AND COALESCE(json_extract(record,'$.payload.by'),'')='finished'))),0) activity,COALESCE((json_extract(record,'$.kind')='watched'),0) watched,COALESCE((json_extract(record,'$.kind')='moved' AND json_extract(record,'$.payload.to')='done'),0) done_move,COALESCE((0),0) agent_comment,COALESCE((json_extract(record,'$.kind')='created' AND json_extract(record,'$.actor.kind')='agent' AND (json_extract(record,'$.mentions_user')=1 OR json_extract(record,'$.notifies_user')=1)),0) mentions
        FROM tracker_events;
        CREATE VIEW IF NOT EXISTS tracker_summary_task AS SELECT id task_id,project_key,number,state,status,created_at,updated_at,record FROM tracker_tasks;
        CREATE VIEW IF NOT EXISTS tracker_summary_metadata AS SELECT t.task_id,json_patch(
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
            FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),'{}')) patch,(EXISTS(SELECT 1 FROM reviews WHERE task_id=t.id AND json_extract(record,'$.mode')='pull_request'
            AND json_type(record,'$.pull_request')='object'
            AND (length(CAST(json_extract(record,'$.pull_request.originating_workspace_id') AS BLOB))>128
                OR length(CAST(json_extract(record,'$.pull_request.latest_published_snapshot_id') AS BLOB))>128))
        OR COALESCE((SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN
            length(CAST(id AS BLOB))>128 OR length(CAST(json_extract(record,'$.target.agent_id') AS BLOB))>128 ELSE 0 END
            FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1),0)) invalid_refs,
            (SELECT CASE WHEN state IN ('waiting','queued','provisioning','failed') THEN
                json_object('kind',substr(json_extract(record,'$.target.kind'),1,32),'agent_id',substr(json_extract(record,'$.target.agent_id'),1,128)) END
                FROM tracker_delivery_intents WHERE task_id=t.id AND state<>'cancelled' ORDER BY generation DESC LIMIT 1) assignment_target
            FROM (SELECT task_id,task_id id FROM tracker_task_summaries) t;
        CREATE VIEW IF NOT EXISTS tracker_summary_dependencies AS SELECT task_id,json_object('blocked',json(CASE WHEN (SELECT state='open' AND status<>'done' FROM tracker_tasks WHERE id=tracker_task_summaries.task_id) AND
        EXISTS(SELECT 1 FROM tracker_task_dependencies d JOIN tracker_tasks b ON b.id=d.waiting_on_task_id
            WHERE d.task_id=tracker_task_summaries.task_id AND b.state<>'closed' AND b.status<>'done') THEN 'true' ELSE 'false' END),
        'waiting_on_count',(SELECT COUNT(*) FROM tracker_task_dependencies WHERE task_id=tracker_task_summaries.task_id),
        'blocks_count',(SELECT COUNT(*) FROM tracker_task_dependencies WHERE waiting_on_task_id=tracker_task_summaries.task_id)) record FROM tracker_task_summaries;
        CREATE VIEW IF NOT EXISTS tracker_summary_identity AS SELECT a.rowid source_rowid,a.id,a.owner_id,json_object('agent_id',a.id,'name',substr(json_extract(a.record,'$.name'),1,128),
        'project_role',substr(json_extract(a.record,'$.project_role'),1,64),'ordinal',a.ordinal,
        'provider',substr(COALESCE(json_extract(a.record,'$.choice.provider'),'claude'),1,64),
        'workspace_id',NULL,'workspace_name',CASE WHEN json_extract(a.record,'$.project_role') IS NULL
            THEN substr(json_extract(r.record,'$.worktree_name'),1,128) END,'available',json('false')) record
        FROM agents a LEFT JOIN implementations r ON r.id=a.owner_id WHERE length(CAST(a.id AS BLOB))<=128;
        CREATE VIEW IF NOT EXISTS tracker_summary_activity AS SELECT c.task_id,
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
        CASE WHEN c.status='done' THEN COALESCE(c.input_done_at,
        (SELECT at FROM tracker_task_summary_facts WHERE task_id=c.task_id AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1),c.created_at) END done_at FROM tracker_task_summary_context c;
        CREATE VIEW IF NOT EXISTS tracker_summary_append AS SELECT task_id,id fact_id,ulid,ts,unread,activity,agent_comment,mentions FROM tracker_task_summary_facts WHERE watched=0;CREATE VIEW IF NOT EXISTS tracker_summary_choices AS
        SELECT c.project_key,'label' dimension,lower(j.value) choice_key,c.task_id,MIN(json_quote(j.value)) value
            FROM tracker_task_summaries c,json_each(c.record,'$.labels') j WHERE NOT json_extract(c.record,'$._summary_invalid_labels')
            GROUP BY c.task_id,lower(j.value)
        UNION ALL SELECT project_key,'label','!invalid:'||task_id,task_id,'null' FROM tracker_task_summaries WHERE json_extract(record,'$._summary_invalid_labels')
        UNION ALL SELECT project_key,'status',json_extract(record,'$.status'),task_id,json_quote(json_extract(record,'$.status')) FROM tracker_task_summaries
        UNION ALL SELECT project_key,'state',json_extract(record,'$.state'),task_id,json_quote(json_extract(record,'$.state')) FROM tracker_task_summaries
        UNION ALL SELECT project_key,'assignee',CASE WHEN value='null' THEN '!invalid:'||task_id ELSE value END,task_id,value FROM (
            SELECT project_key,task_id,CASE WHEN json_extract(context,'$.invalid_assignee') THEN 'null'
                WHEN json_extract(record,'$.assignee') IS NULL THEN json_quote('none')
                WHEN json_extract(record,'$.assignee.kind')='agent' THEN json_object('agent_id',json_extract(record,'$.assignee.agent_id'),'kind','agent')
                ELSE json(json_extract(record,'$.assignee')) END value FROM tracker_task_summaries);CREATE VIEW IF NOT EXISTS tracker_summary_apply_task AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_task_run INSTEAD OF INSERT ON tracker_summary_apply_task BEGIN INSERT INTO tracker_task_summaries(task_id,project_key,number,record,assignment_target,identities,context)
        SELECT t.id,t.project_key,t.number,json_object(
        'id',t.id,'number',t.number,'title',substr(json_extract(t.record,'$.title'),1,1024),
        'state',t.state,'status',substr(t.status,1,128),
        'labels',json(COALESCE((SELECT json_group_array(value) FROM
            (SELECT substr(value,1,40) value FROM json_each(t.record,'$.labels') LIMIT 20)),'[]')),
        'priority',substr(COALESCE(json_extract(t.record,'$.priority'),'none'),1,32),
        'assignee',json(CASE WHEN json_type(t.record,'$.assignee')='object' THEN CASE WHEN json_extract(t.record,'$.assignee.kind')='agent'
            THEN json_object('kind','agent','agent_id',substr(json_extract(t.record,'$.assignee.agent_id'),1,128))
            ELSE json_object('kind',substr(json_extract(t.record,'$.assignee.kind'),1,32)) END END),'created_by',json(CASE WHEN json_type(t.record,'$.created_by')='object' THEN CASE WHEN json_extract(t.record,'$.created_by.kind')='agent'
            THEN json_object('kind','agent','agent_id',substr(json_extract(t.record,'$.created_by.agent_id'),1,128))
            ELSE json_object('kind',substr(json_extract(t.record,'$.created_by.kind'),1,32)) END END),
        'trackers',json(COALESCE((SELECT json_group_array(value) FROM
            (SELECT substr(value,1,128) value FROM json_each(t.record,'$.trackers') LIMIT 32)),'[]')),
        'links',json_object('parent_task_id',substr(json_extract(t.record,'$.links.parent_task_id'),1,128),
            'workspace_ids',json(COALESCE((SELECT json_group_array(value) FROM
                (SELECT substr(value,1,128) value FROM json_each(t.record,'$.links.workspace_ids') LIMIT 20)),'[]'))),
        'attachment_count',COALESCE(json_array_length(t.record,'$.attachments'),0),
        'read_through',substr(json_extract(t.record,'$.read_through'),1,128),
        'watch_started_after',substr(json_extract(t.record,'$.watch_started_after'),1,128),
        'dismissed_through',substr(json_extract(t.record,'$.dismissed_through'),1,128),
        'dependencies',json(COALESCE((SELECT json_extract(s.record,'$.dependencies') FROM tracker_task_summaries s WHERE s.task_id=t.id),'{"blocked":false,"waiting_on_count":0,"blocks_count":0}')),
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
            THEN 'true' ELSE 'false' END)),NULL,COALESCE((SELECT json_group_object(key,json(value)) FROM (SELECT i.key,json_object(
        'agent_id',substr(i.key,1,128),'name',substr(json_extract(i.value,'$.name'),1,128),
        'project_role',substr(json_extract(i.value,'$.project_role'),1,64),
        'ordinal',json_extract(i.value,'$.ordinal'),
        'workspace_id',substr(json_extract(i.value,'$.workspace_id'),1,128),
        'workspace_name',substr(json_extract(i.value,'$.workspace_name'),1,128),
        'provider',substr(json_extract(i.value,'$.provider'),1,64),
        'available',json(CASE WHEN json_extract(i.value,'$.available')=1 THEN 'true' ELSE 'false' END)) value
        FROM json_each(t.record,'$.identities') i
        WHERE length(CAST(i.key AS BLOB))<=128 AND i.key IN (json_extract(t.record,'$.created_by.agent_id'),json_extract(t.record,'$.assignee.agent_id'))
        LIMIT 2)), '{}'),json_object('watched',COALESCE(json_extract(t.record,'$.watched'),0),
        'read_ulid',substr(substr(json_extract(t.record,'$.read_through'),instr(json_extract(t.record,'$.read_through'),'-')+1),1,128),
        'watch_ulid',substr(substr(json_extract(t.record,'$.watch_started_after'),instr(json_extract(t.record,'$.watch_started_after'),'-')+1),1,128),
        'session_start',json_extract(t.record,'$.activity_session.session_started_ms'),
        'session_last',json_extract(t.record,'$.activity_session.last_activity_ms'),
        'done_at',substr(json_extract(t.record,'$.done_at'),1,64),'status',substr(t.status,1,128),
        'created_at',substr(t.created_at,1,64),
        'invalid_assignee',COALESCE(length(CAST(json_extract(t.record,'$.assignee.agent_id') AS BLOB))>128,0))
        FROM (WITH parsed AS MATERIALIZED (SELECT task_id id,project_key,number,state,status,created_at,updated_at,jsonb(record) record
            FROM tracker_summary_task WHERE task_id=NEW.task_id) SELECT * FROM parsed) t WHERE true
        ON CONFLICT(task_id) DO UPDATE SET project_key=excluded.project_key,number=excluded.number,
            record=excluded.record,assignment_target=NULL,identities=excluded.identities,context=excluded.context;INSERT INTO tracker_task_summary_context(task_id,watched,read_ulid,watch_ulid,session_start,session_last,input_done_at,status,created_at)
        SELECT task_id,CASE WHEN json_extract(context,'$.watched')=1 THEN 1 ELSE 0 END,
            json_extract(context,'$.read_ulid'),json_extract(context,'$.watch_ulid'),json_extract(context,'$.session_start'),
            json_extract(context,'$.session_last'),json_extract(context,'$.done_at'),json_extract(context,'$.status'),json_extract(context,'$.created_at')
            FROM tracker_task_summaries WHERE task_id=NEW.task_id
        ON CONFLICT(task_id) DO UPDATE SET watched=excluded.watched,read_ulid=excluded.read_ulid,watch_ulid=excluded.watch_ulid,
            session_start=excluded.session_start,session_last=excluded.session_last,input_done_at=excluded.input_done_at,status=excluded.status,created_at=excluded.created_at
        WHERE watched IS NOT excluded.watched OR read_ulid IS NOT excluded.read_ulid OR watch_ulid IS NOT excluded.watch_ulid OR session_start IS NOT excluded.session_start OR session_last IS NOT excluded.session_last OR input_done_at IS NOT excluded.input_done_at OR status IS NOT excluded.status OR created_at IS NOT excluded.created_at;INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE changes()<>0;INSERT INTO tracker_summary_apply_card(task_id) VALUES(NEW.task_id);INSERT INTO tracker_summary_apply_metadata(task_id) VALUES(NEW.task_id);INSERT INTO tracker_summary_apply_choices(task_id) VALUES(NEW.task_id); END;
CREATE VIEW IF NOT EXISTS tracker_summary_apply_delete AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_delete_run INSTEAD OF INSERT ON tracker_summary_apply_delete BEGIN INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries
            WHERE task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.task_id);
        DELETE FROM tracker_task_summaries WHERE task_id=NEW.task_id;
        DELETE FROM tracker_task_summary_context WHERE task_id=NEW.task_id;
        DELETE FROM tracker_task_summary_facts WHERE task_id=NEW.task_id;
        DELETE FROM tracker_task_filter_choices WHERE task_id=NEW.task_id; END;
CREATE VIEW IF NOT EXISTS tracker_summary_apply_metadata AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_metadata_run INSTEAD OF INSERT ON tracker_summary_apply_metadata BEGIN UPDATE tracker_task_summaries SET (record,assignment_target)=(SELECT
        json_set(json_patch(json_remove(tracker_task_summaries.record,'$.review_summary','$.deferred_assignment'),m.patch),
            '$._summary_invalid_metadata_refs',json(CASE WHEN m.invalid_refs THEN 'true' ELSE 'false' END)),m.assignment_target
        FROM tracker_summary_metadata m WHERE m.task_id=tracker_task_summaries.task_id) WHERE task_id=NEW.task_id; END;
CREATE VIEW IF NOT EXISTS tracker_summary_apply_choices AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_choices_run INSTEAD OF INSERT ON tracker_summary_apply_choices BEGIN DELETE FROM tracker_task_filter_choices AS old WHERE task_id=NEW.task_id AND NOT EXISTS(
        SELECT 1 FROM tracker_summary_choices d WHERE d.task_id=NEW.task_id AND d.project_key=old.project_key AND d.dimension=old.dimension
            AND d.choice_key=old.choice_key AND d.value=old.value);
        INSERT INTO tracker_task_filter_choices(project_key,dimension,choice_key,task_id,value)
        SELECT project_key,dimension,choice_key,task_id,value FROM tracker_summary_choices d WHERE d.task_id=NEW.task_id AND NOT EXISTS(
            SELECT 1 FROM tracker_task_filter_choices old WHERE old.project_key=d.project_key AND old.dimension=d.dimension
                AND old.choice_key=d.choice_key AND old.task_id=d.task_id AND old.value=d.value); END;
CREATE VIEW IF NOT EXISTS tracker_summary_apply_dependencies AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_dependencies_run INSTEAD OF INSERT ON tracker_summary_apply_dependencies BEGIN UPDATE tracker_task_summaries SET record=json_set(record,'$.dependencies',
        json((SELECT record FROM tracker_summary_dependencies d WHERE d.task_id=tracker_task_summaries.task_id))) WHERE task_id=NEW.task_id; END;
CREATE VIEW IF NOT EXISTS tracker_summary_apply_identity AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_identity_run INSTEAD OF INSERT ON tracker_summary_apply_identity BEGIN INSERT INTO tracker_task_summary_identities(agent_id,record) SELECT a.id,a.record FROM tracker_summary_identity a WHERE a.id=NEW.task_id
        ON CONFLICT(agent_id) DO UPDATE SET record=excluded.record WHERE record IS NOT excluded.record; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_task_INSERT AFTER INSERT ON tracker_tasks BEGIN INSERT INTO tracker_summary_apply_task(task_id) VALUES(NEW.id);INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id=NEW.id OR task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_UPDATE AFTER UPDATE ON tracker_tasks
        WHEN OLD.id IS NEW.id AND (OLD.record IS NOT NEW.record OR OLD.project_key IS NOT NEW.project_key OR OLD.number IS NOT NEW.number
            OR OLD.state IS NOT NEW.state OR OLD.status IS NOT NEW.status OR OLD.created_at IS NOT NEW.created_at OR OLD.updated_at IS NOT NEW.updated_at)
        BEGIN INSERT INTO tracker_summary_apply_task(task_id) VALUES(NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_key AFTER UPDATE OF id ON tracker_tasks WHEN OLD.id IS NOT NEW.id
        BEGIN INSERT INTO tracker_summary_apply_delete(task_id) VALUES(OLD.id);INSERT INTO tracker_summary_apply_task(task_id) VALUES(NEW.id);INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id=NEW.id OR task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_dependencies AFTER UPDATE OF state,status ON tracker_tasks
            WHEN OLD.id IS NEW.id AND (OLD.state IS NOT NEW.state OR OLD.status IS NOT NEW.status) BEGIN INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id=NEW.id OR task_id IN (SELECT task_id FROM tracker_task_dependencies WHERE waiting_on_task_id=NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_task_delete AFTER DELETE ON tracker_tasks BEGIN INSERT INTO tracker_summary_apply_delete(task_id) VALUES(OLD.id); END;CREATE VIEW IF NOT EXISTS tracker_summary_apply_activity AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE VIEW IF NOT EXISTS tracker_summary_apply_append AS SELECT CAST(NULL AS TEXT) task_id,CAST(NULL AS TEXT) fact_id WHERE 0;
        CREATE VIEW IF NOT EXISTS tracker_summary_apply_card AS SELECT CAST(NULL AS TEXT) task_id WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_activity_run INSTEAD OF INSERT ON tracker_summary_apply_activity BEGIN
            UPDATE tracker_task_summary_context SET (unread_count,unread_agent_comments,unread_mentions,session_started_ms,last_activity_ms,done_at)=(SELECT unread_count,unread_agent_comments,unread_mentions,session_started_ms,last_activity_ms,done_at
        FROM tracker_summary_activity WHERE task_id=NEW.task_id) WHERE task_id=NEW.task_id; INSERT INTO tracker_summary_apply_card VALUES(NEW.task_id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_card_run INSTEAD OF INSERT ON tracker_summary_apply_card BEGIN UPDATE tracker_task_summaries SET record=(SELECT CASE WHEN c.watched=1 THEN json_set(tracker_task_summaries.record,
        '$.unread_count',c.unread_count,'$.unread_agent_comments',c.unread_agent_comments,'$.unread_mentions',c.unread_mentions,
        '$.session_started_ms',c.session_started_ms,'$.last_activity_ms',c.last_activity_ms,'$.done_at',c.done_at)
        ELSE json_set(json_remove(tracker_task_summaries.record,'$.unread_count','$.unread_agent_comments','$.unread_mentions','$.session_started_ms','$.last_activity_ms'),
            '$.done_at',c.done_at) END FROM tracker_task_summary_context c WHERE c.task_id=NEW.task_id) WHERE task_id=NEW.task_id; END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_append_run INSTEAD OF INSERT ON tracker_summary_apply_append BEGIN
            UPDATE tracker_task_summary_context SET
        unread_count=tracker_task_summary_context.unread_count+(tracker_task_summary_context.watched=1 AND f.unread=1 AND f.ulid>COALESCE(tracker_task_summary_context.read_ulid,'') AND
            f.ulid>COALESCE(tracker_task_summary_context.watch_ulid,(SELECT MAX(ulid) FROM tracker_task_summary_facts WHERE task_id=tracker_task_summary_context.task_id AND watched=1),'')),
        unread_agent_comments=tracker_task_summary_context.unread_agent_comments+(tracker_task_summary_context.watched=1 AND f.agent_comment=1 AND f.ulid>COALESCE(tracker_task_summary_context.read_ulid,'')),
        unread_mentions=tracker_task_summary_context.unread_mentions+(tracker_task_summary_context.watched=1 AND f.mentions=1 AND f.ulid>COALESCE(tracker_task_summary_context.read_ulid,'')),
        session_started_ms=CASE WHEN tracker_task_summary_context.watched=1 THEN CASE WHEN f.activity=1 AND f.ts IS NOT NULL THEN
            CASE WHEN tracker_task_summary_context.session_started_ms IS NULL OR tracker_task_summary_context.last_activity_ms IS NULL OR f.ts-tracker_task_summary_context.last_activity_ms>=28800000 THEN f.ts ELSE tracker_task_summary_context.session_started_ms END
            ELSE tracker_task_summary_context.session_started_ms END END,
        last_activity_ms=CASE WHEN tracker_task_summary_context.watched=1 THEN CASE WHEN f.activity=1 AND f.ts IS NOT NULL THEN f.ts ELSE tracker_task_summary_context.last_activity_ms END END,
        done_at=CASE WHEN tracker_task_summary_context.status='done' THEN COALESCE(tracker_task_summary_context.input_done_at,
        (SELECT at FROM tracker_task_summary_facts WHERE task_id=tracker_task_summary_context.task_id AND done_move=1 ORDER BY at DESC,id DESC LIMIT 1),tracker_task_summary_context.created_at) END FROM tracker_summary_append f WHERE tracker_task_summary_context.task_id=NEW.task_id AND f.task_id=NEW.task_id AND f.fact_id=NEW.fact_id
            AND (tracker_task_summary_context.watched<>1 OR f.activity=0 OR f.ts IS NULL OR tracker_task_summary_context.last_activity_ms IS NULL OR f.ts>=tracker_task_summary_context.last_activity_ms);
            INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE changes()=0;
            INSERT INTO tracker_summary_apply_card VALUES(NEW.task_id); END;
CREATE VIEW IF NOT EXISTS tracker_summary_apply_comment AS SELECT
            CAST(NULL AS TEXT) task_id,CAST(NULL AS TEXT) fact_id,CAST(NULL AS TEXT) old_task_id,
            CAST(NULL AS TEXT) old_fact_id,CAST(NULL AS INTEGER) replace_existing WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_comment_run INSTEAD OF INSERT ON tracker_summary_apply_comment BEGIN
            DELETE FROM tracker_task_summary_facts WHERE (task_id=NEW.task_id AND id=NEW.fact_id)
                OR (task_id=NEW.old_task_id AND id=NEW.old_fact_id);
            INSERT INTO tracker_task_summary_facts(task_id,id,ulid,at,ts,unread,activity,watched,done_move,agent_comment,mentions) SELECT task_id,id,ulid,at,ts,unread,activity,watched,done_move,agent_comment,mentions FROM tracker_summary_comment_facts WHERE id=NEW.fact_id;
            INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE NEW.replace_existing=1 AND NEW.task_id IS NOT NULL
                UNION SELECT NEW.old_task_id WHERE NEW.old_task_id IS NOT NULL;
            INSERT INTO tracker_summary_apply_append SELECT NEW.task_id,NEW.fact_id WHERE NEW.replace_existing=0; END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_comments_INSERT AFTER INSERT ON tracker_comments BEGIN
            INSERT INTO tracker_summary_apply_comment SELECT NEW.task_id,NEW.id,NULL,NULL,
                EXISTS(SELECT 1 FROM tracker_task_summary_facts WHERE task_id=NEW.task_id AND id=NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_comments_UPDATE AFTER UPDATE ON tracker_comments
        WHEN OLD.task_id IS NOT NEW.task_id OR OLD.id IS NOT NEW.id OR OLD.created_at IS NOT NEW.created_at
            OR json_extract(OLD.record,'$.author.kind','$.mentions_user','$.notifies_user') IS NOT json_extract(NEW.record,'$.author.kind','$.mentions_user','$.notifies_user')
        BEGIN INSERT INTO tracker_summary_apply_comment VALUES(NEW.task_id,NEW.id,OLD.task_id,OLD.id,1); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_comments_DELETE AFTER DELETE ON tracker_comments BEGIN
            INSERT INTO tracker_summary_apply_comment VALUES(NULL,NULL,OLD.task_id,OLD.id,1); END;CREATE VIEW IF NOT EXISTS tracker_summary_apply_event AS SELECT
            CAST(NULL AS TEXT) task_id,CAST(NULL AS TEXT) fact_id,CAST(NULL AS TEXT) old_task_id,
            CAST(NULL AS TEXT) old_fact_id,CAST(NULL AS INTEGER) replace_existing WHERE 0;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_apply_event_run INSTEAD OF INSERT ON tracker_summary_apply_event BEGIN
            DELETE FROM tracker_task_summary_facts WHERE (task_id=NEW.task_id AND id=NEW.fact_id)
                OR (task_id=NEW.old_task_id AND id=NEW.old_fact_id);
            INSERT INTO tracker_task_summary_facts(task_id,id,ulid,at,ts,unread,activity,watched,done_move,agent_comment,mentions) SELECT task_id,id,ulid,at,ts,unread,activity,watched,done_move,agent_comment,mentions FROM tracker_summary_event_facts WHERE id=NEW.fact_id;
            INSERT INTO tracker_summary_apply_activity SELECT NEW.task_id WHERE NEW.replace_existing=1 AND NEW.task_id IS NOT NULL
                UNION SELECT NEW.old_task_id WHERE NEW.old_task_id IS NOT NULL;
            INSERT INTO tracker_summary_apply_append SELECT NEW.task_id,NEW.fact_id WHERE NEW.replace_existing=0; END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_events_INSERT AFTER INSERT ON tracker_events BEGIN
            INSERT INTO tracker_summary_apply_event SELECT NEW.task_id,NEW.id,NULL,NULL,
                EXISTS(SELECT 1 FROM tracker_task_summary_facts WHERE task_id=NEW.task_id AND id=NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_events_UPDATE AFTER UPDATE ON tracker_events
        WHEN OLD.task_id IS NOT NEW.task_id OR OLD.id IS NOT NEW.id OR OLD.at IS NOT NEW.at
            OR json_extract(OLD.record,'$.kind','$.actor.kind','$.payload.by','$.payload.waiting_on','$.payload.to','$.mentions_user','$.notifies_user') IS NOT json_extract(NEW.record,'$.kind','$.actor.kind','$.payload.by','$.payload.waiting_on','$.payload.to','$.mentions_user','$.notifies_user')
        BEGIN INSERT INTO tracker_summary_apply_event VALUES(NEW.task_id,NEW.id,OLD.task_id,OLD.id,1); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_events_DELETE AFTER DELETE ON tracker_events BEGIN
            INSERT INTO tracker_summary_apply_event VALUES(NULL,NULL,OLD.task_id,OLD.id,1); END;CREATE TRIGGER IF NOT EXISTS tracker_summary_reviews_INSERT AFTER INSERT ON reviews BEGIN
            INSERT INTO tracker_summary_apply_metadata SELECT NEW.task_id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_reviews_UPDATE AFTER UPDATE ON reviews BEGIN
            INSERT INTO tracker_summary_apply_metadata SELECT OLD.task_id UNION SELECT NEW.task_id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_reviews_DELETE AFTER DELETE ON reviews BEGIN
            INSERT INTO tracker_summary_apply_metadata SELECT OLD.task_id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_delivery_intents_INSERT AFTER INSERT ON tracker_delivery_intents BEGIN
            INSERT INTO tracker_summary_apply_metadata SELECT NEW.task_id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_delivery_intents_UPDATE AFTER UPDATE ON tracker_delivery_intents BEGIN
            INSERT INTO tracker_summary_apply_metadata SELECT OLD.task_id UNION SELECT NEW.task_id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_tracker_delivery_intents_DELETE AFTER DELETE ON tracker_delivery_intents BEGIN
            INSERT INTO tracker_summary_apply_metadata SELECT OLD.task_id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_edges_INSERT AFTER INSERT ON tracker_task_dependencies BEGIN
            INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id IN (NEW.task_id,NEW.waiting_on_task_id); END;CREATE TRIGGER IF NOT EXISTS tracker_summary_edges_UPDATE AFTER UPDATE ON tracker_task_dependencies BEGIN
            INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id IN (NEW.task_id,NEW.waiting_on_task_id,OLD.task_id,OLD.waiting_on_task_id); END;CREATE TRIGGER IF NOT EXISTS tracker_summary_edges_DELETE AFTER DELETE ON tracker_task_dependencies BEGIN
            INSERT INTO tracker_summary_apply_dependencies SELECT task_id FROM tracker_task_summaries WHERE task_id IN (OLD.task_id,OLD.waiting_on_task_id); END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_INSERT AFTER INSERT ON agents BEGIN INSERT INTO tracker_summary_apply_identity(task_id) VALUES(NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_UPDATE AFTER UPDATE ON agents
        WHEN OLD.id IS NOT NEW.id OR OLD.owner_id IS NOT NEW.owner_id OR OLD.ordinal IS NOT NEW.ordinal
            OR json_extract(OLD.record,'$.name','$.project_role','$.choice.provider') IS NOT json_extract(NEW.record,'$.name','$.project_role','$.choice.provider')
        BEGIN DELETE FROM tracker_task_summary_identities WHERE agent_id=OLD.id AND OLD.id<>NEW.id; INSERT INTO tracker_summary_apply_identity(task_id) VALUES(NEW.id); END;
        CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_delete AFTER DELETE ON agents BEGIN
            DELETE FROM tracker_task_summary_identities WHERE agent_id=OLD.id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_implementations_INSERT AFTER INSERT ON implementations  BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE owner_id=NEW.id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_implementations_UPDATE AFTER UPDATE ON implementations WHEN OLD.id IS NOT NEW.id OR json_extract(OLD.record,'$.worktree_name') IS NOT json_extract(NEW.record,'$.worktree_name') BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE owner_id IN (OLD.id,NEW.id); END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_implementations_DELETE AFTER DELETE ON implementations  BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE owner_id=OLD.id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_tasks_INSERT AFTER INSERT ON tasks  BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE owner_id=NEW.id; END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_tasks_UPDATE AFTER UPDATE ON tasks WHEN OLD.id IS NOT NEW.id OR json_extract(OLD.record,'$.worktree_name') IS NOT json_extract(NEW.record,'$.worktree_name') BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE owner_id IN (OLD.id,NEW.id); END;CREATE TRIGGER IF NOT EXISTS tracker_summary_identity_tasks_DELETE AFTER DELETE ON tasks  BEGIN
            INSERT INTO tracker_summary_apply_identity SELECT id FROM agents WHERE owner_id=OLD.id; END;