# Fresh addendum disposition

Independent Sol architecture reviewer `/root/architecture_review` reviewed
ADDENDUM.md, the consumer witnesses and #892's actual task/approved intent.
Verdict: **approve for design handoff**. It checked pre-S version/COW coverage,
finite physical-key/as-of access, one #892 owner, no long reader or retrospective
copy, explicit pressure retirement, immutable caller/card generations, and the
conditional 409/1,099/1,573-turn and 625MiB arithmetic. It confirmed that new
costs/envelope remain unmeasured and implementation is not authorized.

A final recheck also confirmed the distinction between #900 opaque basis, retained
source S and separately retained journal replay; no approval expanded.

A wording correction replaced "existing" #892 retention worker/admission with
"planned ... once implemented". The canonical SQL revision-publication boundary
is also explicitly a required property, not an existing #892 API.

Independent Sol scope reviewer `/root/scope_audit` rechecked those corrections
and the consumer gap witnesses. Verdict: **approve narrow design handoff**, no
remaining scope findings. Neither reviewer ran builds, tests or timing.

These dispositions approve no task dispatch, dependency mutation, new service,
production API/implementation, supported-envelope/cost acceptance, timing run or
#891 activation. Epic coordination and implementation decisions remain pending.
The original research review/evidence at 821a605438 is unchanged.

Validation of this addendum: three new domain tests pass; the full isolated
suite passes 25 tests; the actual Node Number/JSON witness exits 0; explicit-path
semgrep exits 0. Commands, red/green tests and original-packet immutability proof
are in addendum-evidence/. No retained-generation prototype or new performance
measurement was run. Node v26.12.1 was used only for the ECMAScript Number/JSON
semantic witness, not as a SPA gate or runtime-policy change.
