# #722 handover to #773

This is a preservation-only snapshot of local runner scripts, notes, held implementation patches, calibration evidence and control preparation. No code implementation, build or test was performed for this handover. The 43 controls remain unrun; all prepared implementations remain held for the recorded causal test gates. Task #722 is not qualified.

The archive preserves `.task722-artifacts/` paths relative to the workspace parent of `Build/`. Existing originals are still present there. The bulk historical raw gate logs/binaries remain local, not in this archive; runner scripts and gate summary notes are included. `manifest.json` hashes every archived file. Do not restore over newer work blindly.

For a clean recovery from the repository root, extract with:

```sh
tar -xzf handover/task722/local-runner-and-notes.tar.gz -C ..
```

Read `.task722-artifacts/ROOT-CHECKPOINT.md`, `preparation/checkpoint010/README.txt`, and the #773 runbook first. Candidate source was c15ff338 before this documentation-only commit. Both execution authorization fields are deliberately null. The incoming operator must record explicit coordinator release and rebind the candidate/source hashes to the final authorized commit before preparing a new artifact root. Do not bypass those guards; do not reuse interrupted checkpoint009. Native/Large/originals retain their separately recorded product-dependency holds.

After authorization and source rebinding, from this repository root:

```sh
qualify_sha=$(git rev-parse HEAD)
python3 ../.task722-artifacts/preparation/checkpoint010/prepare-frozen-root.py --frozen-sha "$qualify_sha" --counter 10
qualify_root="../.task722-artifacts/gates/release-checkpoint-010-${qualify_sha:0:12}/controls"
python3 ../.task722-artifacts/preparation/narrow-release-after-bae/at-ni10.py python3 ../.task722-artifacts/preparation/checkpoint010/run-exact.py build --root "$qualify_root" --profile release
python3 ../.task722-artifacts/preparation/narrow-release-after-bae/at-ni10.py python3 ../.task722-artifacts/preparation/checkpoint010/run-staged-exact.py "$qualify_root"
```

These are instructions only; none were executed. They run the independent 43-control checkpoint, not the six original trials. The #773 operator owns all subsequent execution, cleanup and qualification. The runner aborts on wrapper expiry, retains the actual result, marks remaining controls NOT RUN and requires an owned PID/start cleanup audit. A prerequisite failure is not intended-assertion RED evidence.
