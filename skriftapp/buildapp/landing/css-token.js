export function readNumericToken(element, tokenName) {
  return parseFloat(getComputedStyle(element).getPropertyValue(tokenName));
}
