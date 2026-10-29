# #925 selection-build research packet

Read [DESIGN.md](DESIGN.md) for the proposed contract and
[EVIDENCE.md](EVIDENCE.md) for measured limits, refusal/progress results and
unimplemented production requirements. [REVIEW.md](REVIEW.md) records the
independent review disposition. [evidence/commands.txt](evidence/commands.txt)
pins commands and exits; [evidence/provenance.json](evidence/provenance.json)
pins actual measured source and environment.

This directory is design/isolated prototype work only. It creates no production
API, capability, selection service, activation, migration or SPA dispatch.

Run the research tests from the repository root:

```sh
nice -n 10 python -m unittest discover -s planning/v2/task-925-selection -p 'test_prototype.py' -v
```

The largest proposed quiet workload and sixteen-job cohort fail the finite
modeled service deadline. A passing test suite does not accept those availability
limits, producer costs or production performance. Final evidence deliberately
keeps pending/refusal distinct from exact zero.
