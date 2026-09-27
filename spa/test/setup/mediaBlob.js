// jsdom does not implement object URLs. Give DOM tests an address with the
// browser's scheme while leaving Chromium's native implementation untouched.
if (typeof URL.createObjectURL !== "function") {
  let next = 0;
  URL.createObjectURL = () => `blob:test-${++next}`;
  URL.revokeObjectURL = () => {};
}
