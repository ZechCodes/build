export const TIMING_TABLE = `| Condition | Before | After |
| --- | ---: | ---: |
| A write holding the app lock | 100–149 ms | 0.5–2.5 ms |
| A git operation running | 105–178 ms | 0.4–2.3 ms |
| Nothing else running | 0.8–28.5 ms | 0.3–1.3 ms |`;

export const MARKDOWN_POLISH_FIXTURE = `# Markdown, made readable

A release note with **clear priorities**, *a little emphasis*, and ~~obsolete advice~~. Run \`npm test\`, then press <kbd>Ctrl</kbd> + <kbd>Enter</kbd>.

## Timing at a glance

${TIMING_TABLE}

Measurements stay together, even in a narrow conversation.[^timing]

### Lists with structure

3. Review the change
   - Check desktop and phone widths
   - [x] Keep raw HTML inert
   - [ ] Publish after review
4. Share the evidence

> A regular quotation can contain **formatted text**.
>
> - Nested context stays readable.

## Useful signals

> [!NOTE]
> Measurements come from the same fixture in both themes.

> [!TIP]
> Use a table to compare before and after.

> [!IMPORTANT]
> Keep short values and their units on one line.

> [!WARNING]
> A wide table scrolls inside the message.

> [!CAUTION]
> Review the source before running unfamiliar commands.

## Code that reads like code

\`\`\`javascript
const timings = [0.5, 2.5];
const fastest = Math.min(...timings);
console.log({ fastest, unit: "ms" });
\`\`\`

~~~text
Unknown languages and <script>markup</script> stay literal.
~~~

### Links and images

[Project website](https://example.com "Example") · <https://example.com/docs> · hello@example.com

![A quiet mountain landscape](https://markdown-fixture.test/landscape.svg "Fixture image")

<details>
<summary>How we checked the result</summary>

The same source exercises **every feature** at desktop and phone widths.

- [x] Light theme
- [x] Dark theme

</details>

---

#### Smaller headings

Level four still reads as a heading.

##### Supporting notes

Level five keeps the hierarchy.

###### Fine print

Raw HTML remains text: <img src=x onerror=alert(1)>.

[^timing]: All values are milliseconds. The table reports a range, not a single sample.
`;

export const FIXTURE_IMAGE = `<svg xmlns="http://www.w3.org/2000/svg" width="720" height="180" viewBox="0 0 720 180"><rect width="720" height="180" fill="#d5e7e0"/><path d="M0 180 160 38 310 180 480 60 720 180" fill="#87aea0"/><path d="m180 180 190-106 175 106" fill="#4c7b6a"/><circle cx="592" cy="48" r="19" fill="#fbf5d5"/></svg>`;
export const FIXTURE_STYLES = `html, body { height:auto; overflow:auto; } body { padding:24px; } .fixture { width:100%; min-width:0; max-width:760px; margin:auto; padding:0; } .fixture-heading { font-size:11px; font-weight:650; letter-spacing:.08em; text-transform:uppercase; color:var(--dim); margin:0 0 12px; } @media(max-width:500px) { body { padding:12px; } }`;
export const fixtureMarkup = (html) => `<main class="fixture"><p class="fixture-heading">Build · Markdown review</p><div class="thread-message agent"><div class="thread-comment-card"><div class="thread-body markdown">${html}</div></div></div></main>`;
