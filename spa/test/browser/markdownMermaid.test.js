import { afterAll, beforeAll, expect, it } from "vitest";
import { loadBrowserModules, mountLayout, openLayoutSession } from "./layoutHarness.mjs";
let session;
beforeAll(async () => { session = await openLayoutSession({ plugins: [{ name: "mermaid-test-dependencies", config: () => ({ optimizeDeps: { include: ["mermaid"] } }) }] }); });
afterAll(async () => { await session?.close(); });
const diagrams = [
  ["flowchart", "flowchart LR\nStart --> Finish", ["Start", "Finish"]],
  ["sequence", "sequenceDiagram\nAlice->>Bob: Hello\nBob-->>Alice: Welcome", ["Alice", "Bob", "Hello", "Welcome"]],
  ["class", "classDiagram\nAnimal <|-- Dog\nAnimal : +age\nDog : +bark()\nCar *-- Wheel\nCar o-- Driver", ["Animal", "Dog", "age", "bark"]],
  ["state", "stateDiagram-v2\n[*] --> Idle\nIdle --> Active\nActive --> [*]", ["Idle", "Active"]],
  ["pie", 'pie title Work\n"Done" : 60\n"Remaining" : 40', ["Work", "Done", "Remaining"]],
];

async function setup(page, basePath) {
  await mountLayout(page, '<main class="markdown" style="width:360px"></main>', { basePath });
  await loadBrowserModules(page, { md: "src/core/markdown.js", diagrams: "src/core/markdownMermaid.js", theme: "src/core/theme.js", renderer: "src/core/mermaidRenderer.js" }, basePath);
  await page.evaluate(() => {
    window.__diagramDispose = window.__layoutModules.diagrams.installMarkdownMermaid(document, { load: async () => ({ renderMermaid: (...args) => window.__layoutModules.renderer.renderMermaid(...args).catch((error) => { window.__diagramError = String(error); throw error; }) }) });
    window.__setDiagram = (source) => {
      window.__diagramError = null;
      document.querySelector("main").innerHTML = window.__layoutModules.md.markdownHtml(`\`\`\`mermaid\n${source}\n\`\`\``);
    };
  });
}
for (const [name, source, labels] of diagrams) {
  it(`renders readable ${name} geometry in both themes from cached source`, async () => {
    await session.withPage(async ({ page, basePath }) => {
      await setup(page, basePath);
      await page.evaluate((source) => window.__setDiagram(source), source);
      await page.waitForFunction(() => document.querySelector(".md-mermaid > svg") || window.__diagramError, null, { timeout: 30_000 });
      expect(await page.evaluate(() => window.__diagramError)).toBeNull();
      const inspect = () => page.evaluate(() => {
        const svg = document.querySelector(".md-mermaid > svg");
        return { text: svg.textContent, box: [svg.getBoundingClientRect().width, svg.getBoundingClientRect().height], color: getComputedStyle(svg.querySelector("text")).fill, unsafe: svg.querySelectorAll("style, foreignObject, image, a, script, [style]").length, paths: svg.querySelectorAll("path, rect, circle, polygon").length, sourceHidden: document.querySelector("pre").hidden };
      });
      if (name === "sequence") {
        const semantics = await page.evaluate(() => ["solid", "dashed"].map((kind) => getComputedStyle(document.querySelector(`.md-svg-message-${kind}`)).strokeDasharray));
        expect(semantics[0]).toBe("none");
        expect(semantics[1]).not.toBe("none");
      }
      if (name === "pie") {
        const palette = await page.evaluate(() => [...document.querySelectorAll("svg > g path[class^=md-svg-slice]")].map((slice) => getComputedStyle(slice).fill));
        expect(new Set(palette).size).toBe(2);
        expect(palette).not.toContain("none");
      }
      if (name === "class") {
        const fills = await page.evaluate(() => ["hollow", "filled"].map((kind) => getComputedStyle(document.querySelector(`marker.md-svg-marker-${kind} path`)).fill));
        expect(fills[0]).not.toBe(fills[1]);
        const circles = await page.locator("marker.md-svg-marker-hollow circle").count();
        expect(circles).toBeGreaterThan(0);
        expect(await page.locator("marker circle.md-svg-dot").count()).toBe(0);
      }
      if (name === "state") expect(await page.locator(".md-svg-dot").count()).toBeGreaterThan(0);
      const light = await inspect();
      for (const label of labels) expect(light.text).toContain(label);
      expect(light.box[0]).toBeGreaterThan(100);
      expect(light.box[0]).toBeLessThanOrEqual(360);
      expect(light.box[1]).toBeGreaterThan(20);
      expect(light.paths).toBeGreaterThan(0);
      expect(light.unsafe).toBe(0);
      expect(light.sourceHidden).toBe(true);
      const oldId = await page.$eval(".md-mermaid > svg", (svg) => svg.id);
      await page.evaluate(() => window.__layoutModules.theme.applyTheme("dark"));
      await page.waitForFunction((id) => document.querySelector(".md-mermaid > svg")?.id !== id, oldId);
      const dark = await inspect();
      expect(dark.color).not.toBe(light.color);
      for (const label of labels) expect(dark.text).toContain(label);
      expect(await page.locator(".md-mermaid-measure").count()).toBe(0);
    });
  }, 60_000);
}

it("loads no Mermaid runtime for ordinary markdown and rejects hostile transient-DOM resources without requests", async () => {
  await session.withPage(async ({ page, basePath }) => {
    const external = [];
    const libraryRequests = [];
    page.on("request", (request) => {
      if (!request.url().startsWith("http://127.0.0.1:")) external.push(request.url());
      if (request.url().includes("/node_modules/mermaid/") || /deps\/mermaid[_.]/.test(request.url())) libraryRequests.push(request.url());
    });
    await page.route("https://evil.test/**", (route) => route.abort());
    await setup(page, basePath);
    await page.evaluate(() => { document.querySelector("main").innerHTML = window.__layoutModules.md.markdownHtml("```js\nconst n = 1;\n```"); });
    expect(libraryRequests).toEqual([]);
    const hostile = [
      '%%{init: {"securityLevel":"loose","themeCSS":"@import url(https://evil.test/config)"}}%%\nflowchart LR\nA-->B',
      '---\nconfig:\n securityLevel: loose\n themeCSS: "@import url(https://evil.test/yaml)"\n---\nflowchart LR\nA-->B',
      'flowchart LR\nA["<img src=https://evil.test/html onerror=window.pwned=1>"]',
      'flowchart LR\nA["&lt;img src=https://evil.test/entity&gt;"]',
      'flowchart LR\nA@{ img: "https://evil.test/shape" }',
      'flowchart LR\nA-->B\nclassDef default fill:url(https://evil.test/css)',
      'flowchart LR\nA-->B\nstyle A fill:url(https://evil.test/style)',
      'flowchart LR\nA-->B\nclick A "javascript:window.pwned=1"',
      'flowchart LR\nA["\\x3cimg src=https://evil.test/escape"]',
      'this is invalid Mermaid',
    ];
    for (const source of hostile) {
      await page.evaluate((source) => window.__setDiagram(source), source);
      await page.waitForSelector(".md-mermaid-error");
      expect(await page.locator("pre").isVisible()).toBe(true);
      expect(await page.locator("code").textContent()).toBe(`${source}\n`);
      expect(await page.locator("main svg, main img, .md-mermaid-measure").count()).toBe(0);
    }
    // A valid render exercises the measurement path under the same observer.
    await page.evaluate(() => window.__setDiagram("flowchart LR\nA-->B"));
    await page.waitForFunction(() => document.querySelector(".md-mermaid > svg") || window.__diagramError, null, { timeout: 30_000 });
      expect(await page.evaluate(() => window.__diagramError)).toBeNull();
    expect(await page.evaluate(() => window.pwned)).toBeUndefined();
    expect(external).toEqual([]);
    expect(libraryRequests.length).toBeGreaterThan(0);
  });
}, 60_000);

it("rejects sequence actor metadata before any transient image href assignment or resource request", async () => {
  await session.withPage(async ({ page, basePath }) => {
    const resources = [];
    page.on("request", (request) => {
      if (request.resourceType() === "image" || !request.url().startsWith("http://127.0.0.1:")) resources.push(request.url());
    });
    await page.route("**/*", (route) => {
      if (route.request().resourceType() === "image" || !route.request().url().startsWith("http://127.0.0.1:")) return route.abort();
      return route.fallback();
    });
    await setup(page, basePath);
    await page.evaluate(() => {
      window.__imageAssignments = [];
      const record = (element, name, value) => {
        if (["image", "img"].includes(element.localName) && /(?:href|src)$/.test(name)) window.__imageAssignments.push({ name, value, connected: element.isConnected });
      };
      const set = Element.prototype.setAttribute;
      Element.prototype.setAttribute = function(name, value) { record(this, name, value); return set.call(this, name, value); };
      const setNS = Element.prototype.setAttributeNS;
      Element.prototype.setAttributeNS = function(ns, name, value) { record(this, name, value); return setNS.call(this, ns, name, value); };
      const src = Object.getOwnPropertyDescriptor(HTMLImageElement.prototype, "src");
      Object.defineProperty(HTMLImageElement.prototype, "src", { ...src, set(value) { record(this, "src", value); src.set.call(this, value); } });
    });
    for (const metadata of [
      'properties Alice: {"icon":"//evil.test/pixel"}',
      'properties Alice: {"icon":"/relative/pixel"}',
      'details Alice: {"properties":{"icon":"//evil.test/details"}}',
      'details Alice: {"properties":{"icon":"./legacy/pixel"}}',
    ]) {
      const source = `sequenceDiagram\nparticipant Alice\nparticipant Bob\n${metadata}\nAlice->>Bob: Hello`;
      await page.evaluate((source) => window.__setDiagram(source), source);
      await page.waitForFunction(() => document.querySelector(".md-mermaid > svg, .md-mermaid-error"), null, { timeout: 30_000 });
      expect(await page.evaluate(() => window.__imageAssignments)).toEqual([]);
      expect(await page.locator(".md-mermaid-error").textContent()).toBe("Diagram unavailable");
      expect(await page.locator("pre").isVisible()).toBe(true);
    }
    expect(resources).toEqual([]);
  });
}, 60_000);
