// Known instants in the ordered-query domain must match ECMAScript Date.parse.
// Unsupported cases record a boundary; Date.parse may still accept them.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const fixture = JSON.parse(readFileSync(new URL('../fixtures/api/task-query-timestamps.json', import.meta.url), 'utf8'));
let supported = 0;
let browserAcceptedOutsideDomain = 0;
for (const sample of fixture.cases) {
  if (sample.outcome === 'known') {
    assert.equal(Date.parse(sample.input), sample.millis, sample.input);
    supported++;
  } else if (sample.outcome === 'unsupported' && Number.isFinite(Date.parse(sample.input))) {
    browserAcceptedOutsideDomain++;
  }
}
assert.ok(supported >= 13);
assert.ok(browserAcceptedOutsideDomain > 0, 'fixture must witness conservative refusal');
console.log(`${process.version}: ${supported} supported instants match Date.parse; ${browserAcceptedOutsideDomain} browser-accepted inputs deliberately refused`);
