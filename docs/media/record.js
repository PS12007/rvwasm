// Records the README media from a locally served build of web/.
//
//   ./scripts/build-web.sh && python3 -m http.server -d web 8123 &
//   node docs/media/record.js http://localhost:8123 /tmp/rvwasm-media
//   then convert with docs/media/make-gifs.sh /tmp/rvwasm-media
//
// Needs Playwright with Chromium. If the machine can't reach the jsDelivr CDN,
// set XTERM_DIR to a directory with node_modules/@xterm/{xterm,addon-fit}.
const { chromium } = require('playwright');
const path = require('path');
const fs = require('fs');

const base = process.argv[2] || 'http://localhost:8123';
const out = process.argv[3] || '/tmp/rvwasm-media';
fs.mkdirSync(out, { recursive: true });
const size = { width: 1280, height: 760 };

async function context(browser, video) {
  const ctx = await browser.newContext({
    viewport: size,
    deviceScaleFactor: 1,
    colorScheme: 'dark',
    ...(video ? { recordVideo: { dir: out, size } } : {}),
  });
  if (process.env.XTERM_DIR) {
    await ctx.route(/cdn\.jsdelivr\.net\/npm\/(.*)/, (route) => {
      const m = route.request().url().match(/npm\/(@xterm\/[^@]+)@[^/]+\/(.*)/);
      route.fulfill({ path: path.join(process.env.XTERM_DIR, 'node_modules', m[1], m[2]) });
    });
  }
  return ctx;
}

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
    await p.waitForFunction(() => document.querySelector('.xterm-rows')?.textContent.includes('$'), null, { timeout: 30000 });
    await p.click('#terminal');
    await p.waitForTimeout(600);
    await type(p, 'ls\n', 1200);
    await type(p, 'echo hello from xv6 > note\n', 400);
    await type(p, 'cat note\n', 700);
    await type(p, 'forktest\n', 1800);
    await type(p, 'grep -n Unix README\n', 900);
    await type(p, 'wc README\n', 1500);
  });

  await clip(browser, 'linux', `${base}/index.html?guest=linux`, async (p) => {
    await p.waitForFunction(() => document.querySelector('.xterm-rows')?.textContent.includes('root@rvwasm'), null, { timeout: 60000 });
    await p.click('#terminal');
    await p.waitForTimeout(500);
    await type(p, 'uname -a\n', 700);
    await type(p, 'head -5 /proc/cpuinfo\n', 900);
    await type(p, 'ls /bin | head -24 | xargs\n', 1200);
    await type(p, 'top\n', 3500);
    await type(p, 'q', 1000);
  });

  // Timeline viewer with a trace recorded from the page.
  const ctx = await context(browser, false);
  const page = await ctx.newPage();
  await page.goto(`${base}/index.html`);
  await page.waitForFunction(() => document.querySelector('.xterm-rows')?.textContent.includes('$'), null, { timeout: 30000 });
  // Record while usertests is busy, so the trace has user/kernel switches.
  await page.selectOption('#trace-cap', '200000');
  await page.click('#terminal');
  await page.keyboard.type('usertests -q\n');
  await page.waitForTimeout(4000);
  await page.click('#trace-btn');
  await page.waitForTimeout(1500);
  await page.click('#trace-btn');
  await page.waitForFunction(() => !document.querySelector('#trace-open').disabled);
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
