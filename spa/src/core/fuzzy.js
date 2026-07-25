// Subsequence matching for the branch picker.
//
// Branch names are long, structured and share prefixes (feat/…, fix/…), so a
// plain "contains" makes you type the noise before the signal. A subsequence
// match lets "fcm" find feat/core-moderation, while the scoring keeps the
// obvious answers on top: an exact name first, then a prefix, then how tightly
// and how early the letters landed. Ties fall back to the caller's order, which
// for branches is most-recently-committed first — recency beats brevity when
// two names match a query equally well.

/** Score `text` against `query`, or null when the query is not a subsequence.
 *  Higher is better. Case-insensitive. */
export function fuzzyScore(text, query) {
  const haystack = String(text || "");
  const needle = String(query || "").trim();
  if (!needle) return 0;
  const lowerHaystack = haystack.toLowerCase();
  const lowerNeedle = needle.toLowerCase();

  if (lowerHaystack === lowerNeedle) return 10000;
  if (lowerHaystack.startsWith(lowerNeedle)) return 5000;

  let score = 0;
  let at = 0;
  let previousIndex = -1;
  for (const ch of lowerNeedle) {
    const found = lowerHaystack.indexOf(ch, at);
    if (found === -1) return null;
    // Adjacent letters are worth more than scattered ones, and a match at a
    // word boundary (after / - _ .) reads as intentional.
    if (found === previousIndex + 1) score += 12;
    if (found === 0 || /[/\-_.]/.test(lowerHaystack[found - 1] || "")) score += 8;
    score += Math.max(0, 6 - (found - at)); // earlier is better
    previousIndex = found;
    at = found + 1;
  }
  // No length tiebreak: for branches the caller's order is recency, and a
  // more recent branch is a better guess than a shorter one. Equal scores fall
  // through to that order in fuzzyRank.
  return score;
}

/**
 * Rank `items` against `query`, keeping the caller's order for ties and
 * dropping non-matches. An empty query keeps the original order untouched.
 * `key` reads the string to match on (default: the item itself).
 */
export function fuzzyRank(items, query, key = (item) => item) {
  const list = items || [];
  if (!String(query || "").trim()) return [...list];
  return list
    .map((item, index) => ({ item, index, score: fuzzyScore(key(item), query) }))
    .filter((entry) => entry.score !== null)
    .sort((a, b) => b.score - a.score || a.index - b.index)
    .map((entry) => entry.item);
}
