// Which way a peer connection is carrying, read off a stats report.
import { describe, expect, it } from "vitest";
import { classifyTransportPath } from "../src/core/transportPath.js";

/** A stats report as a browser hands one over: a Map of entries by id. */
const report = (entries) => new Map(entries.map((entry) => [entry.id, entry]));

const pair = (over = {}) => ({
  id: "pair-1",
  type: "candidate-pair",
  state: "succeeded",
  nominated: true,
  localCandidateId: "local-1",
  remoteCandidateId: "remote-1",
  ...over,
});

const local = (candidateType) => ({ id: "local-1", type: "local-candidate", candidateType });
const remote = (candidateType) => ({ id: "remote-1", type: "remote-candidate", candidateType });

const pathOf = (entries) => classifyTransportPath(report(entries));

describe("the path a connection is carrying on", () => {
  it.each([
    ["relay", "host", "direct"],
    ["host", "relay", "turn"],
  ])("uses the transport's current %s to %s selection when old nominated pairs remain", (oldType, selectedType, expected) => {
    const entries = [
      pair(), local(oldType), remote("host"),
      pair({ id: "pair-2", localCandidateId: "local-2", remoteCandidateId: "remote-2" }),
      { id: "local-2", type: "local-candidate", candidateType: selectedType },
      { id: "remote-2", type: "remote-candidate", candidateType: "host" },
      { id: "transport-1", type: "transport", selectedCandidatePairId: "pair-2" },
    ];
    expect(pathOf(entries)).toBe(expected);
  });

  it("uses nomination when the transport's selected pair is missing from the report", () => {
    expect(pathOf([
      pair(), local("relay"), remote("host"),
      { id: "transport-1", type: "transport", selectedCandidatePairId: "missing" },
    ])).toBe("turn");
  });

  it("finds a known selected pair after a transport whose selected pair is missing", () => {
    expect(pathOf([
      pair({ nominated: false }), local("host"), remote("host"),
      { id: "transport-0", type: "transport", selectedCandidatePairId: "missing" },
      { id: "transport-1", type: "transport", selectedCandidatePairId: "pair-1" },
    ])).toBe("direct");
  });
  it("is direct when both ends are on the machine itself", () => {
    expect(pathOf([pair(), local("host"), remote("host")])).toBe("direct");
  });

  it("is direct through a reflexive candidate, which is still the machine", () => {
    // srflx is the machine's own address as the world sees it: the traffic
    // goes to the machine, not through anybody.
    expect(pathOf([pair(), local("srflx"), remote("host")])).toBe("direct");
    expect(pathOf([pair(), local("host"), remote("prflx")])).toBe("direct");
  });

  it("is TURN when either end is a relay", () => {
    expect(pathOf([pair(), local("relay"), remote("host")])).toBe("turn");
    expect(pathOf([pair(), local("host"), remote("relay")])).toBe("turn");
    expect(pathOf([pair(), local("relay"), remote("relay")])).toBe("turn");
  });

  it("reads the pair the transport points at where none is marked nominated", () => {
    const entries = [
      pair({ nominated: false, state: "succeeded" }),
      { id: "transport-1", type: "transport", selectedCandidatePairId: "pair-1" },
      local("relay"),
      remote("host"),
    ];
    expect(pathOf(entries)).toBe("turn");
  });

  it("ignores the pairs that lost", () => {
    const entries = [
      pair({ id: "pair-0", state: "failed", nominated: false, localCandidateId: "local-0" }),
      { id: "local-0", type: "local-candidate", candidateType: "relay" },
      pair(),
      local("host"),
      remote("host"),
    ];
    expect(pathOf(entries)).toBe("direct");
  });

  it("says nothing it cannot read", () => {
    expect(pathOf([])).toBe(null);
    expect(pathOf([pair()]), "a pair whose candidates are not in the report").toBe(null);
    expect(pathOf([pair(), local(undefined), remote("host")])).toBe(null);
    expect(classifyTransportPath(null)).toBe(null);
  });

  it("reads a report handed over as a plain list or object, as a test fixture is", () => {
    const entries = [pair(), local("relay"), remote("host")];
    expect(classifyTransportPath(entries)).toBe("turn");
    expect(classifyTransportPath(Object.fromEntries(entries.map((entry) => [entry.id, entry])))).toBe("turn");
  });
});
