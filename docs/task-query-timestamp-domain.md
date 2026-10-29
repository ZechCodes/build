# Ordered task-query timestamp domain

`ecmascript-iso-v1` is the deterministic timestamp domain for new ordered-query
facts. It is a conservative supported subset with an explicit native-producer
fraction extension. It is not a replacement for `Date.parse`, and does not
change legacy history/session parsing or legacy task reads.

The parser accepts complete Gregorian dates `YYYY-MM-DD`, including year zero,
and signed six-digit expanded years `+YYYYYY-MM-DD` / `-YYYYYY-MM-DD` except
`-000000`. Date-only input means UTC midnight. A date-time must append uppercase
`THH:mm:ss`, an optional fraction, and an explicit uppercase `Z` or numeric
`+HH:mm` / `-HH:mm` offset. Hours are 00–23, minutes/seconds 00–59, offset hours
00–23, and offset minutes 00–59. Dates must exist in the proleptic Gregorian
calendar. Leap seconds, overflow-day normalization, `24:00`, shortened dates,
zone-less times, whitespace, lowercase variants and informal formats are outside
this domain. The final UTC instant must lie within inclusive ±8,640,000,000,000,000
milliseconds; offset conversion occurs before this range check.

Fractions accept one through nine decimal digits because native
`store::now_rfc3339` writes the `time` crate's RFC3339 representation, which can
contain submillisecond precision. The parser retains the first three fractional
digits, padding shorter fractions on the right. It never rounds the final epoch
value toward zero: `1969-12-31T23:59:59.999999999Z` means -1 millisecond.

The ECMAScript [Date Time String Format](https://tc39.es/ecma262/multipage/numbers-and-dates.html#sec-date-time-string-format)
standardizes three fractional digits. Its [Date.parse rules](https://tc39.es/ecma262/multipage/numbers-and-dates.html#sec-date.parse)
permit implementation-specific handling outside that format, and distinguish
UTC date-only input from local zone-less date-times. The [TimeClip rule](https://tc39.es/ecma262/multipage/numbers-and-dates.html#sec-timeclip)
supplies the range boundary. Accepting fractional widths other than three is a
tested producer-compatibility extension; these sources do not establish identical
behavior for every browser implementation.

`parse(None)` and `parse(Some(""))` yield `Unknown`; callers map JSON null to
None. Any other nonempty input outside the domain yields `Unsupported`.
Whitespace is unsupported. Integration must retain the distinction: unsupported
nonempty values cannot silently become zero, null or a fallback timestamp.
Nonstring JSON values require caller rejection. The projection owns refusal
metadata and indexed project preflight; this module neither scans projects nor
decides which field an order needs. Counts, exact relations and number lookup
remain independent from timestamp ordering support.

Shared cases live in `fixtures/api/task-query-timestamps.json` and are consumed
by `bridge/tests/task_query_timestamps.rs` and
`scripts/test-task-query-timestamps.mjs`. Run the latter with Node 22.23.4:

```
node scripts/test-task-query-timestamps.mjs
```

On 2026-10-29, Node v22.23.4 and v26.12.1 independently matched all 31 supported
fixture instants. An isolated local HTML evaluation in Chromium
152.0.7977.82 (Arch Linux; headless user-agent Chrome/152.0.0.0) matched all 31
with zero errors. Fixtures include every fractional width 1–9, pre-epoch values,
positive/negative offsets, expanded years, leap-year cases, and both TimeClip
boundaries including offset conversion. Thirteen refused fixture strings were
nevertheless accepted by those engines: refusal deliberately marks a narrower
domain. These witnesses establish the tested engine behavior, not support for
arbitrary browsers or arbitrary timestamp syntax.

The standalone Rust tests were observed failing with the initial stub, then
passing after implementation. The native fraction regression failed against the
initial exactly-three-digit parser and passed after expanding the declared
profile. A standalone `rustc --test` wrapper additionally consumed every shared
fixture and passed all three tests without invoking the shared Cargo build.
