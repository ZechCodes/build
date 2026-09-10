export function readToken(element, tokenName) {
  return getComputedStyle(element).getPropertyValue(tokenName).trim();
}

export function readNumericToken(element, tokenName) {
  return parseFloat(readToken(element, tokenName));
}
