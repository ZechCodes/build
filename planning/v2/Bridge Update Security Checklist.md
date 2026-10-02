# Bridge update security checklist

**Status:** verified (10/10 controls)

**Scope:** replacing a development build with a release from the app, and the
development build's checks (#322). The release-build update path (signed
`SHA256SUMS`, archive digest, staged digest and version probe, the detached
helper, probation and rollback) is unchanged here and not rescored; controls
6 and 7 record that the replacement goes through it and that a release
install keeps every check it had.

## Threat model

A development build is any binary the install marker does not vouch for: a
source build run by the service, a source build copied over a release, a
bridge started by hand. Until #322 none could be updated from the app. Now a
person can choose to put a release in place of one. What must not follow:

- an old client, a stray call, or a replayed request replacing a build the
  person did not choose to replace, including an install queued before the
  binary became a development build;
- the helper writing anywhere but the running bridge's own executable, or
  writing an executable the service will not run;
- a failed replacement leaving the machine in a state it was not in before
  (a marker vouching for a binary it did not vouch for, or a lost one);
- the replacement weakening any check a release install makes;
- a development build reaching the network on its own.

## Controls

| # | Control | Status | Required evidence |
| --- | --- | --- | --- |
| 1 | A development build installs only on a request that carries `replace_development_build: true`. A request without it, or with `false`, is refused, so no client from before wire 3.5.0 can trigger it. A queued install ("when agents are done") saves the confirmation with its attempt and stages or launches nothing on a development build without it: a schedule queued by a release build, or for an older release, is dropped when the bridge starts or the release changes, never run. | [x] | `replaceable_development_build_installs_only_when_the_replacement_is_confirmed`; `replacing_a_development_build_needs_the_confirmation_on_the_request` (absent and `false` answer `conflict`); `a_release_build_s_schedule_does_not_replace_a_development_build_after_restart`; `a_development_build_that_cannot_be_replaced_drops_a_saved_schedule`; `a_confirmed_replacement_schedule_is_saved_and_survives_restart`; `a_newer_release_needs_its_own_confirmation_to_replace_a_development_build`; `stage_scheduled_locked` checks `schedule_confirmed` before it stages. |
| 2 | Only a development binary the platform service runs (its unit's exact `ExecStart`/`ProgramArguments`) is replaceable. A bridge started by hand, or a service that starts another executable, is refused even with the confirmation. | [x] | `development_binary_is_not_replaceable_unless_the_service_runs_it`; `development_build_checks_when_asked_but_does_not_install`; `development_build_can_check_but_cannot_install` (RPC `unavailable` with the confirmation). |
| 3 | The provenance the request was confirmed for is checked again where the helper is staged and launched, not only when the service starts. | [x] | `launch_checks_the_provenance_the_job_was_confirmed_for`; `ProductionBackend::stage` and `installer::launch` both call `provenance::installable_binary`. |
| 4 | The helper writes only the running bridge's own executable: the service's main PID must be this bridge, and the running image must match the file on disk byte for byte, before the service stops. | [x] | `a_replacement_refuses_a_development_build_rebuilt_since_it_started`; `install` checks `validate_manager_pid` first (`manager_pid_must_match_before_any_swap`). |
| 5 | The release is downloaded and verified by the same path as a release update: cosign over `SHA256SUMS`, the archive digest, the staged digest and version. | [x] | `ProductionBackend::stage` is shared; `release_signed_by_another_owner_or_for_another_tag_is_rejected`, `signed_digest_must_match_exact_archive`, `changed_staged_bytes_are_rejected_before_version_probe_runs`. |
| 6 | A failed replacement restores the development binary, the task store and the marker exactly as they were: none stays none, a stale marker comes back byte for byte. | [x] | `a_failed_replacement_restores_the_development_build_and_no_marker`; `a_failed_replacement_restores_a_stale_marker_byte_for_byte`; `an_empty_saved_marker_restores_as_no_marker`. |
| 7 | A release install keeps every marker check: without a marker, or with a saved marker that does not vouch for the binary, it is refused before the service stops. | [x] | `a_release_install_without_a_marker_is_refused_before_the_service_stops`; `a_release_install_keeps_rejecting_a_stale_saved_marker`; `marker_must_match_running_binary_and_service`. |
| 8 | A successful replacement leaves a marker for the release, so later updates take the release path with its checks. | [x] | `replacing_an_unmarked_development_build_marks_the_release`. |
| 9 | The SPA sends the confirmation only from the warning that names the release and says the bridge restarts and local source changes leave the running bridge; Install alone sends nothing. | [x] | `deviceSettingsDom.test.js` "replaces a development build only after its warning is confirmed"; `bridgeUpdateLayout.test.js` "shows the replacement warning on a development build the app can replace". |
| 10 | A development build makes no release check of its own, at startup or daily, and nothing in the SPA marks it as having an update. | [x] | `development_build_never_checks_on_its_own`; `deviceSettingsDom.test.js` "marks nothing for a development build, whatever its cached status says"; `settingsModal.test.js` "does not mark a development build in the settings sidebar". |
