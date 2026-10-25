// Called again inside animation frame: a mutation-time match is not a paint.
export const paintedRowMatches = (row, sequence, body) => Boolean(row?.isConnected
  && row.dataset.sequence === String(sequence) && typeof body === "string"
  && row.querySelector(".thread-body")?.textContent.trim() === body);

export function paintOrderMatches(expected, painted, domHighwaters) {
  return expected.length === painted.length && painted.length === domHighwaters.length
    && expected.every((sequence, index) => sequence === painted[index])
    && domHighwaters.every((sequence, index) => sequence >= painted[index]
      && (index === 0 || sequence >= domHighwaters[index - 1]));
}
