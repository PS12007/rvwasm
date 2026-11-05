// Records the README media from a locally served build of web/.
//
//   ./scripts/build-web.sh && python3 -m http.server -d web 8123 &
//   node docs/media/record.js http://localhost:8123 /tmp/rvwasm-media
//   then convert with docs/media/make-gifs.sh /tmp/rvwasm-media
//
// Needs Playwright with Chromium. If the machine can't reach the jsDelivr CDN,
// set XTERM_DIR to a directory with node_modules/@xterm/{xterm,addon-fit,addon-webgl}.
const { chromium } = require('playwright');
const path = require('path');
const fs = require('fs');

const base = process.argv[2] || 'http://localhost:8123';
const out = process.argv[3] || '/tmp/rvwasm-media';
fs.mkdirSync(out, { recursive: true });
const size = { width: 1440, height: 860 };

async function context(browser, video) {
  const ctx = await browser.newContext({
    viewport: size,
    deviceScaleFactor: 1,
    ...(video ? { recordVideo: { dir: out, size } } : {}),
  });
  // Skip the first-visit tour and keep the command drawer open.
  await ctx.addInitScript(() => {
    localStorage.setItem('rvwasm-toured', '1');
    localStorage.setItem('rvwasm-drawer', 'open');
  });
  if (process.env.XTERM_DIR) {
    await ctx.route(/cdn\.jsdelivr\.net\/npm\/(.*)/, (route) => {
      const m = route.request().url().match(/npm\/(@xterm\/[^@]+)@[^/]+\/(.*)/);
      route.fulfill({ path: path.join(process.env.XTERM_DIR, 'node_modules', m[1], m[2]) });
    });
  }
  return ctx;
}

const booted = (p) => p.waitForFunction(() => document.querySelector('#boot').hidden, null, { timeout: 90000 });
const command = (p, text) => p.locator('.cmd', { has: p.locator(`code:text-is("${text}")`) }).click();

async function type(page, text, wait = 900) {
  await page.keyboard.type(text, { delay: 55 });
  await page.waitForTimeout(wait);
}

async function clip(browser, name, url, script) {
  const ctx = await context(browser, true);
  const page = await ctx.newPage();
  await page.goto(url);
  await script(page);
  const video = page.video();
  await ctx.close();
  fs.renameSync(await video.path(), path.join(out, `${name}.webm`));
}

(async () => {
  const browser = await chromium.launch();

  await clip(browser, 'xv6', `${base}/index.html`, async (p) => {
    await booted(p);
    await p.waitForTimeout(600);
    await command(p, 'ls');
    await p.waitForTimeout(1400);
    await command(p, 'echo hi > a.txt');
    await p.waitForTimeout(500);
    await command(p, 'cat a.txt');
    await p.waitForTimeout(900);
    await p.click('#terminal');
    await type(p, 'forktest\n', 1800);
    await command(p, 'wc README');
    await p.waitForTimeout(1500);
  });

  await clip(browser, 'linux', `${base}/index.html?guest=linux`, async (p) => {
    await booted(p);
    await p.waitForTimeout(500);
    await command(p, 'uname -a');
    await p.waitForTimeout(800);
    await p.click('#terminal');
    await type(p, 'head -5 /proc/cpuinfo\n', 900);
    await command(p, 'top');
    await p.waitForTimeout(3500);
    await p.click('#terminal');
    await type(p, 'q', 1000);
  });

  // A recording, its plain-English summary, and the timeline viewer.
  const ctx = await context(browser, false);
  const page = await ctx.newPage();
  await page.goto(`${base}/index.html`);
  await booted(page);
  await page.selectOption('#trace-cap', '1000000');
  await page.click('#trace-btn');
  await command(page, 'cat README');
  await page.waitForFunction(() => !document.querySelector('#trace-open').disabled, null, { timeout: 30000 });
  await page.waitForTimeout(600);
  const rec = page.locator('#rec-module');
  await rec.scrollIntoViewIfNeeded();
  await rec.screenshot({ path: path.join(out, 'recorder.png') });
  await page.screenshot({ path: path.join(out, 'console.png') });

  const [tl] = await Promise.all([ctx.waitForEvent('page'), page.click('#trace-open')]);
  await tl.setViewportSize(size);
  await tl.waitForSelector('#main:not([hidden])');
  await tl.waitForTimeout(800);
  const ev = tl.locator('.ev:has(.k.exception)').nth(3);
  if (await ev.count()) await ev.click();
  const box = await tl.locator('#tl').boundingBox();
  await tl.mouse.move(box.x + box.width / 2, box.y + 150);
  for (let i = 0; i < 3; i++) { await tl.mouse.wheel(0, -260); await tl.waitForTimeout(80); }
  await tl.mouse.move(box.x + box.width * 0.55, box.y + 210);
  await tl.waitForTimeout(400);
  await tl.screenshot({ path: path.join(out, 'timeline.png') });
  await ctx.close();

  await browser.close();
})();
