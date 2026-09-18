// Rebuild the social card from the Blender hero render and approved HTML type.
import {fileURLToPath} from "node:url";
import {chromium} from 'playwright';
const browser=await chromium.launch({executablePath:process.env.CHROMIUM_PATH});
const page=await browser.newPage({viewport:{width:1200,height:630},deviceScaleFactor:1});
await page.goto(process.env.LANDING_URL || 'http://127.0.0.1:4173/',{waitUntil:'networkidle'});
await page.setContent(`<!doctype html><html><head><style>@font-face{font-family:Inter;src:url('/landing/fonts/inter-700-latin.woff2')}*{box-sizing:border-box}body{margin:0;background:#050706;color:#f5f8f6;font-family:Inter,Arial,sans-serif;overflow:hidden}.brand{position:absolute;top:48px;left:60px;display:flex;align-items:center;gap:12px;font-size:24px}.brand img{width:24px;height:24px}h1{position:absolute;left:60px;top:165px;width:470px;font-size:66px;letter-spacing:-3px;line-height:1.03;margin:0}.device{position:absolute;width:690px;right:-26px;top:155px}.alpha{position:absolute;bottom:64px;left:60px;color:#61f6b0;font-size:16px;letter-spacing:.1em}</style></head><body><div class="brand"><img src="/landing/brand-mark.svg" alt="">build_</div><h1>Set the work<br>in motion.</h1><img class="device" src="/landing/assets/devices/hero-laptop.webp" alt=""><div class="alpha">FREE &amp; OPEN SOURCE · ALPHA</div></body></html>`);
await page.evaluate(()=>Promise.all([document.fonts.ready,...[...document.images].map(i=>i.decode())]));
await page.screenshot({path:fileURLToPath(new URL('../skriftapp/buildapp/landing/assets/social-preview.png',import.meta.url))});
await browser.close();
