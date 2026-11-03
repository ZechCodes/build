#!/usr/bin/env bash
# Each control owns its copied source until a verified restore, including on
# timeout or interruption. No git checkout/stash and no installed daemon.
set -euo pipefail

repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
scratch=/home/zech/Projects/.agent-scratch/977
export CARGO_BUILD_JOBS=8
export CARGO_TARGET_DIR="$scratch/target"
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
mkdir -p "$scratch/evidence/controls"

if [[ ${1:-} != --one ]]; then
  for mechanism in rtc_admission family_admission memory_pool cancellation drop_order revocation drain rtc_drain idle_wake reap_lock transport_overlap push_order push_duplicate; do
    nice -n 10 timeout --signal=TERM --kill-after=15 900 bash "$0" --one "$mechanism"
  done
  exit 0
fi

mechanism=$2
expected_failure=
case "$mechanism" in
  transport_overlap|push_order|push_duplicate)
    source_file=bridge/src/http/runtime/tests/subscriptions/cross_transport/load.rs
    test_name=cross_transport_full_stream_pool_preserves_progress_and_bounds
    case "$mechanism" in
      transport_overlap) expected_failure='REST must return 200 while all four RTC handlers remain pending' ;;
      push_order) expected_failure='SSE pushes preserve exact sequence without duplicates' ;;
      push_duplicate) expected_failure='exactly three pushes: no trailing duplicate remains queued' ;;
    esac ;;
  rtc_admission)
    source_file=bridge/src/carrier.rs
    test_name=cross_transport_rtc_admission_is_bounded_without_starving_rest ;;
  family_admission|memory_pool)
    source_file=bridge/src/http/limits.rs
    test_name=cross_transport_full_stream_pool_preserves_progress_and_bounds ;;
  cancellation)
    source_file=bridge/src/execution/session.rs
    test_name=cross_transport_cancel_calls_retains_running_work_then_releases ;;
  drop_order)
    source_file=bridge/src/http/limits.rs
    test_name=buffer_reservation_releases_family_before_returning_pool_permits ;;
  revocation)
    source_file=bridge/src/http/listener.rs
    test_name=cross_transport_revocation_cuts_stalled_output_within_one_recheck ;;
  drain)
    source_file=bridge/src/http/runtime.rs
    test_name=cross_transport_update_drain_fences_openings_and_joins_streams ;;
  rtc_drain)
    source_file=bridge/src/liveness.rs
    test_name=cross_transport_update_drain_fences_openings_and_joins_streams ;;
  idle_wake)
    source_file=bridge/src/http/sse_stream.rs
    test_name=cross_transport_idle_revocation_within_one_authority_recheck ;;
  reap_lock)
    source_file=bridge/src/http/runtime.rs
    test_name=cross_transport_reaper_notification_can_reenter_cleanup ;;
  *) exit 2 ;;
esac
cd "$repo"
backup=$(mktemp "$scratch/977-control-${mechanism}-XXXXXX")
cp -- "$source_file" "$backup"
restore() {
  cp -- "$backup" "$source_file"
  cmp --silent -- "$backup" "$source_file"
  rm -- "$backup"
}
trap restore EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
python - "$source_file" "$mechanism" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
mutations = {
    "transport_overlap": (
        "        // Only release RTC work after every REST response in this batch.\n",
        "        // Negative control: finish every RTC call before REST starts.\n"
        "        gate.release();\n"
        "        for (index, client) in f.rtc.iter_mut().enumerate() {\n"
        "            client.reply(batch * RTC_SESSIONS + index).await;\n"
        "        }\n"),
    "push_order": (
        "        for sequence in 0..3 {\n            sender.push",
        "        for sequence in [1, 0, 2] {\n            sender.push"),
    "push_duplicate": (
        "        for sequence in 0..3 {\n            sender.push",
        "        for sequence in [0, 1, 2, 2] {\n            sender.push"),
    "rtc_admission": ("const ADMISSION_DEPTH: usize = 1024;", "const ADMISSION_DEPTH: usize = 2048;"),
    "family_admission": ("if count >= MAX_FAMILY_STREAMS {", "if count > MAX_FAMILY_STREAMS {"),
    "memory_pool": ("acquire(&self.stream_bytes, super::sse::MAX_QUEUED_BYTES)?", "acquire(&self.stream_bytes, 0)?"),
    "cancellation": ("self.reply.cancellation.cancel();", "// negative control: request drop does not cancel interest"),
    "drop_order": (
        "    _family: Option<FamilyStream>,\n    _permits: Vec<OwnedSemaphorePermit>,",
        "    _permits: Vec<OwnedSemaphorePermit>,\n    _family: Option<FamilyStream>,"),
    "revocation": ("std::time::Duration::from_millis(25);", "std::time::Duration::from_millis(250);"),
    "drain": ("        self.state.retire_all()", "        0 // negative control: leave openings alive"),
    "rtc_drain": ("runtime.shutdown_background();", "std::mem::forget(runtime); // negative control: keep the driver alive"),
    "idle_wake": ("                _ = self.recheck.tick() => {}\n", ""),
    "reap_lock": (
        "        // Never retain the openings registry lock across that handoff.\n",
        "        // Never retain the openings registry lock across that handoff.\n        let _registry = self.openings.lock().unwrap();\n"),
}
before, after = mutations[sys.argv[2]]
text = path.read_text()
assert text.count(before) == 1, f"control needs one exact mutation in {path}"
path.write_text(text.replace(before, after))
PY
log="$scratch/evidence/controls/$mechanism.log"
set +e
nice -n 10 cargo test --manifest-path bridge/Cargo.toml --lib "$test_name" -- --nocapture > "$log" 2>&1
code=$?
set -e
printf '%s: mutated test exit=%s\n' "$mechanism" "$code"
[[ $code == 101 ]]
# A compile error is not an observed mechanism failure.
rg -q "$test_name .*FAILED" "$log"
if [[ -n $expected_failure ]]; then
  rg -Fq "$expected_failure" "$log"
fi
cmp --silent -- "$backup" "$source_file" && exit 3
restore
trap - EXIT
printf '%s: restored and cmp verified\n' "$mechanism"
