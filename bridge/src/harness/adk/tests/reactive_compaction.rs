//! Fake protocol for the bounded recovery path statically inspected in 2.1.284.
//! The fake proves wrapper behavior; the qualification doc records source evidence.
use super::*;
use crate::compaction::{CompactionSource, ResolvedCompactionPolicy};
use crate::harness::compaction::{CompactionLaunchIdentity, PolicySession};

#[tokio::test]
async fn native_reactive_compaction_recovers_or_reports_the_original_wake_once() {
    for succeeds in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("reactive.py");
        std::fs::write(&script, r#"
import json,os,sys
args=sys.argv[1:]
assert args[args.index('--resume')+1]=='sess-adk'
settings=json.loads(args[args.index('--settings')+1])
assert settings['autoCompactEnabled'] is True
assert settings['env']['DISABLE_AUTO_COMPACT']=='0'
assert os.environ['DISABLE_AUTO_COMPACT']=='0'
def emit(value):
    print(json.dumps(value),flush=True)
for index,line in enumerate(sys.stdin):
    with open(os.environ['INPUT_LOG'],'a') as log: log.write(line)
    if index==0:
        emit({'type':'system','subtype':'init','session_id':'sess-adk','model':'claude-fable-5-1'})
        emit({'type':'assistant','error':'invalid_request','isApiErrorMessage':True,'message':{'model':'<synthetic>','content':[{'type':'text','text':'Prompt is too long'}]}})
        emit({'type':'system','subtype':'status','status':'compacting'})
        if os.environ['RECOVERY_SUCCEEDS']=='0':
            emit({'type':'system','subtype':'status','status':None,'compact_result':'failed'})
            emit({'type':'result','subtype':'success','is_error':True,'result':'Prompt is too long'})
            continue
        emit({'type':'system','subtype':'compact_boundary','compact_metadata':{'trigger':'auto','pre_tokens':210000,'post_tokens':8000}})
    emit({'type':'assistant','message':{'content':[{'type':'text','text':'Continued original request'}]}})
    emit({'type':'result','subtype':'success','is_error':False})
"#).unwrap();
        let choice = adk_choice();
        let policy = AdkHarness
            .prepare_compaction_policy(
                ResolvedCompactionPolicy {
                    tokens: 200_000,
                    source: CompactionSource::Conversation("reactive".into()),
                },
                choice.clone(),
                Some("2.1.284".into()),
                CompactionLaunchIdentity::new("reactive"),
            )
            .unwrap();
        let mut options = spawn_options();
        options.resume_session_id = Some("sess-adk".into());
        options.compaction_policy = Some(policy.clone());
        let mut spec = AdkHarness.spec(&choice, &options, &context()).unwrap();
        spec.binary = "python3".into();
        spec.args.insert(0, script.to_string_lossy().into_owned());
        spec = spec
            .env("RECOVERY_SUCCEEDS", if succeeds { "1" } else { "0" })
            .env(
                "INPUT_LOG",
                directory.path().join("input").to_string_lossy(),
            );
        let (session, mut activity) = AdkSession::spawn(&spec, None, &choice).unwrap();
        let session = PolicySession::attach(Arc::new(session), policy);
        let status = session.status_changed().unwrap();
        session
            .send_turn(&Turn::new("first wake after deploy").with_operation_id("original-wake"))
            .unwrap();
        assert!(becomes_true_within(Duration::from_secs(5), || status
            .borrow()
            .status
            == AgentStatus::Waiting));
        let reports = reports_already_sent(&mut activity);
        assert_eq!(
            reports
                .iter()
                .filter(|report| matches!(&report.activity,
            AgentActivity::TurnFailed { operation_id: Some(id), .. } if id == "original-wake"))
                .count(),
            usize::from(!succeeds)
        );
        assert_eq!(
            reports.iter().any(|report| matches!(
                report.activity,
                AgentActivity::Compaction { completed: true }
            )),
            succeeds
        );
        assert!(
            !session.automatic_compaction_due(Some(900_000)),
            "native owns the threshold"
        );
        session
            .send_turn(&Turn::new("later input").with_operation_id("later-wake"))
            .unwrap();
        assert!(becomes_true_within(Duration::from_secs(5), || status
            .borrow()
            .status
            == AgentStatus::Waiting));
        let written = std::fs::read_to_string(directory.path().join("input")).unwrap();
        let turns: Vec<Value> = written
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(turns.len(), 2, "Build sends no replay or /compact");
        assert_eq!(
            turns[0]["message"]["content"][0]["text"],
            "first wake after deploy"
        );
        assert_eq!(turns[1]["message"]["content"][0]["text"], "later input");
        session.end();
    }
}
