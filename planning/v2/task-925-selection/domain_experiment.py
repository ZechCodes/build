"""Deterministic refusal-impact model; not an execution of Build's Rust Store.

Fixtures preserve canonical values and mimic the documented lossy source fields.
They test the design's validation rule; production parity remains a delivery gate.
"""
import json


def source_projection(task):
    labels = task["labels"]
    status = task["status"]
    agent = task.get("agent")
    invalid = set()
    if len(labels) > 20 or any(len(x.encode()) > 40 for x in labels):
        invalid.add("labels")
        labels = []
    if len(status.encode()) > 128:
        invalid.add("status")
        status = "\0unsupported-status"
    if agent is not None and len(agent.encode()) > 128:
        invalid.add("agent")
        agent = None
    if any(x == "" or x != x.strip(" ") for x in task["labels"]):
        invalid.add("labels")
    return dict(task, labels=labels, status=status, agent=agent, invalid=invalid)


def refusal(task, projection, state):
    # State is independently lossless here. No predicate on an unknown field is
    # used to exclude a candidate. Every result stays private until scan/seal.
    if task["state"] != state:
        return False
    return bool(projection["invalid"])


def run():
    fixtures = [
        dict(state="closed", status="backlog", labels=[f"l{i}" for i in range(21)]),
        dict(state="closed", status="x" * 129, labels=[]),
        dict(state="closed", status="y" * 129, labels=[]),
        dict(state="closed", status="backlog", labels=["é" * 21]),
        dict(state="closed", status="backlog", labels=[], agent="a" * 129),
        dict(state="closed", status="é" * 65, labels=[]),
        dict(state="closed", status="backlog", labels=[""]),
        dict(state="closed", status="backlog", labels=[" padded "]),
    ]
    projected = list(map(source_projection, fixtures))
    assert all(p["invalid"] for p in projected)
    assert projected[1]["status"] == projected[2]["status"]
    assert projected[0]["labels"] == []
    assert projected[4]["agent"] is None
    assert projected[6]["labels"] == [""]
    assert projected[7]["labels"] == [" padded "]
    for status in ["x\0y", "é" * 64]:
        p = source_projection(dict(state="open", status=status, labels=[]))
        assert not p["invalid"] and p["status"] == status
    rows = []
    for state in ["open", "closed"]:
        conservative = sum(any(p["invalid"] for p in projected) for _ in range(10))
        candidate = sum(any(refusal(t, p, state) for t, p in zip(fixtures, projected))
                        for _ in range(10))
        # Queries differ only in lossless irrelevant ID/label selectors in this
        # count model. They all exclude/include these six tasks by state first.
        rows.append(dict(selected_state=state, admitted_queries=10,
                         indexed_project_refusals=conservative,
                         complete_scan_refusals=candidate,
                         exact_normalized_future_facts_query_refusals=10 if state == "closed" else 0))
    # Future exact label relations support high cardinality/longer <=128B facts;
    # proposed 128B opaque status/actor ceiling still refuses the other 4 rows.
    return dict(kind="deterministic model, no wall timing", fixtures=len(fixtures),
                canonical_values_unchanged=True, projection_alias_detected=True,
                validation="off-page scan must finish or refuse before any publication",
                scenarios=rows, unsupported_candidates_current=8, unsupported_candidates_future=6,
                caveat="Synthetic incidence, not a rate estimate for user projects; Future facts recover two label candidates; four opaque-field candidates still exceed the proposed byte ceiling. Two malformed-label candidates also remain unsupported. These closed-state query fixtures cannot safely prune them.")


if __name__ == "__main__":
    print(json.dumps(run(), indent=2))
