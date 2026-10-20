// An indented list body is read in its own bounded block context. Document
// prepasses skip it so its fences and delimiters cannot consume or close
// blocks beyond the list. The nested renderer reads that body separately.
export function listBodyEnd(lines, at) {
  const item = /^( *)(?:[-*+]|\d{1,9}[.)]) +/.exec(lines[at]);
  if (!item) return at;
  let next = at + 1;
  while (next < lines.length) {
    if (lines[next].trim() && lines[next].length - lines[next].trimStart().length <= item[1].length) break;
    next += 1;
  }
  return next - 1;
}
