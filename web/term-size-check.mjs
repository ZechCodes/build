// Confirms ghostty's grid matches the PTY's size (the cause of TUI garbling).
import { spawn } from "node:child_process";
import { chromium } from "playwright";

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const SCREEN = () => {
  const b = window.term?.buffer?.active;
  if (!b) return "";
  let s = "";
  for (let i = 0; i < b.length; i++) { const ln = b.getLine(i); if (ln) s += ln.translateToString(true) + "\n"; }
  return s;
};

const server = spawn("node", ["serve.mjs"], { env: { ...process.env, PORT: "18099" }, stdio: "ignore" });
await sleep(800);
const browser = await chromium.launch({ headless: true });
const page = await browser.newPage({ viewport: { width: 1400, height: 800 } });
try {
  await page.goto("http://localhost:18099/terminal.html", { waitUntil: "load" });
  await page.waitForFunction(() => document.getElementById("status")?.textContent === "connected", null, { timeout: 25000 });
  await sleep(1200);

  const ghostty = await page.evaluate(() => ({ cols: window.term.cols, rows: window.term.rows }));
  await page.evaluate(() => window.session.input("echo SZ=$(tput cols)x$(tput lines)\n"));
  await sleep(1000);
  const screen = await page.evaluate(SCREEN);
  const m = screen.match(/SZ=(\d+)x(\d+)/);
  const pty = m ? { cols: +m[1], rows: +m[2] } : null;

  console.log(`ghostty grid: ${ghostty.cols}x${ghostty.rows}`);
  console.log(`PTY (tput):   ${pty ? pty.cols + "x" + pty.rows : "??"}`);
  const ok = pty && ghostty.cols === pty.cols && ghostty.rows === pty.rows;
  console.log(ok ? "✓ MATCH — TUI layout will render correctly" : "✗ MISMATCH — TUIs will garble");
  process.exitCode = ok ? 0 : 1;
} finally {
  await browser.close();
  server.kill();
}
