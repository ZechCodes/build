// Synthetic sparse retained history, not a reconstruction of live membership.
// Counter 1100 updated creation 1000; only nineteen new creations follow 1080.
const message = (sequence) => ({ type: "message", data: { id: `history-${sequence}`,
  sequence, role: "agent", body: `historical fixture ${sequence} ${"r".repeat(1000)}` } });

// Swap two existing slots to burst the mounted tips without changing load or
// any conversation's sequence set. No response delay is introduced.
export function historyReplayFrame(frame) {
  const ordered = frame === 1 ? 6 : frame === 6 ? 1 : frame;
  return { rowIndex: ordered % 6, sequence: 1100 + Math.floor(ordered / 6) + 1 };
}

export function historyRepairSeed() {
  const sequences = [...Array.from({ length: 280 }, (_, index) => index + 1),
    ...Array.from({ length: 50 }, (_, index) => 296 + index * 16)];
  const held = sequences.map(message);
  const history = Array.from({ length: 1080 }, (_, index) => message(index + 1));
  history[999] = { ...history[999], data: { ...history[999].data, updated_sequence: 1100,
    body: "historically updated retained message" } };
  return { held, history, backlog: Array.from({ length: 19 }, (_, index) => message(1081 + index)) };
}

export function historyRepairPage(seed, arriving, params) {
  const bySequence = new Map([...seed.history, ...seed.backlog, ...arriving]
    .map((item) => [item.data.sequence, item]));
  const all = [...bySequence.values()].sort((left, right) => left.data.sequence - right.data.sequence);
  const eligible = all.filter((item) => item.data.sequence > Number(params.after_sequence || 0));
  const limit = Number(params.limit || 100);
  return { items: params.newest ? eligible.slice(-limit) : eligible.slice(0, limit),
    thread_last_sequence: Math.max(1100, ...all.map((item) => Math.max(item.data.sequence, item.data.updated_sequence || 0))),
    has_more: eligible.length > limit };
}
