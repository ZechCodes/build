//! Actual encrypted smoke collector. Final source test calls these
//! around real ReadyReadInputs::shutdown and RpcHarness::finish, so assertions
//! cannot skip child/carrier cleanup. Does not synthesize any response.
use super::{full_seed::ReadyReadInputs, rpc_full, FullReadOutcome, FullRpcCase};
use crate::app::tests::runtime::write_wait_qualification::rpc::{ReplyProof, RpcHarness};
use std::collections::{BTreeMap, BTreeSet};

pub(in crate::app::tests::runtime::write_wait_qualification) async fn collect_native_rpc_reads(
    rpc: &RpcHarness,
    ready: &ReadyReadInputs,
) -> Vec<(FullRpcCase, ReplyProof)> {
    ready.assert_pinned_footprint(rpc);
    let cases = ready.rpc_cases();
    let mut replies = Vec::with_capacity(cases.len());
    for (index, case) in cases.into_iter().enumerate() {
        let id = format!("qual/fixture/full-read/{index}/{}", case.method);
        let proof = rpc
            .request(index % 4, &id, case.method, case.params.clone())
            .await;
        replies.push((case, proof));
    }
    // worktree.diff and project/workspace reads may exercise adoption discovery;
    // do not hide an unexpected registry import of the support checkout.
    ready.assert_pinned_footprint(rpc);
    replies
}

pub(in crate::app::tests::runtime::write_wait_qualification) fn verify_native_rpc_reads(
    ready: &ReadyReadInputs,
    replies: &[(FullRpcCase, ReplyProof)],
) {
    let mut completed = BTreeMap::new();
    for (case, proof) in replies {
        assert_eq!(proof.reply["id"], proof.original_id);
        assert_eq!(proof.method, case.method);
        let outcome = ready.assert_rpc_reply(case, &proof.reply);
        let expected = match case.form {
            rpc_full::Form::Ready => FullReadOutcome::Ready,
            rpc_full::Form::LegacyFull => FullReadOutcome::LegacyFull,
        };
        assert_eq!(
            outcome, expected,
            "{} did not return its native saved value: {}",
            case.method, proof.reply
        );
        assert!(
            completed.insert(case.method, outcome).is_none(),
            "duplicate actual method completion"
        );
    }
    // Actual case-specific wire oracles precede executed-method equality.
    let expected = super::rpc::registered_methods();
    let actual = completed.keys().copied().collect::<BTreeSet<_>>();
    assert_eq!(expected.len(), 45, "audit actual read registry drift");
    assert_eq!(
        actual, expected,
        "every deployed read needs a verified actual wire completion"
    );
}
