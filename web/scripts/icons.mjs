// Renders the app icons in web/public from the SVGs in web/icons.
//
//   npm run icons
//
// Chrome draws an app window's taskbar button from the page's 16 px icon,
// enlarged (to 24 px at 100% scaling), whatever larger sizes the page
// offers. So the 16 px icon is drawn on its own pixel grid (logo-16.svg),
// and favicon.ico carries it as an exact frame: given only a 32 px PNG,
// Chrome would shrink that to 16 px itself and blur it first.

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const web = join(dirname(fileURLToPath(import.meta.url)), "..");
const svg = (name) => readFileSync(join(web, "icons", name), "utf8");

// favicon.ico: 16 px for normal screens, 32 and 48 px for sharper ones.
const ICO = [
  { size: 16, from: "logo-16.svg" },
  { size: 32, from: "logo.svg" },
  { size: 48, from: "logo.svg" },
];
const PNG = [{ file: "apple-touch-icon.png", size: 180, from: "logo.svg" }];

const browser = await chromium.launch({ channel: process.env.E2E_CHANNEL ?? "chrome", headless: true });
const page = await browser.newPage({ deviceScaleFactor: 1 });

async function render(size, from) {
  await page.setViewportSize({ width: size, height: size });
  const sized = svg(from).replace("<svg ", `<svg width="${size}" height="${size}" style="display:block" `);
  await page.setContent(`<!doctype html><html><body style="margin:0;background:transparent">${sized}</body></html>`);
  return page.screenshot({ omitBackground: true, clip: { x: 0, y: 0, width: size, height: size } });
}

for (const o of PNG) {
  writeFileSync(join(web, "public", o.file), await render(o.size, o.from));
  console.log(`public/${o.file}`);
}

// An .ico is a directory of PNGs.
const frames = [];
for (const f of ICO) frames.push({ size: f.size, data: await render(f.size, f.from) });
const head = Buffer.alloc(6 + 16 * frames.length);
head.writeUInt16LE(0, 0);
head.writeUInt16LE(1, 2);
head.writeUInt16LE(frames.length, 4);
let offset = head.length;
frames.forEach((f, n) => {
  const e = 6 + 16 * n;
  head.writeUInt8(f.size, e);
  head.writeUInt8(f.size, e + 1);
  head.writeUInt16LE(1, e + 4);
  head.writeUInt16LE(32, e + 6);
  head.writeUInt32LE(f.data.length, e + 8);
  head.writeUInt32LE(offset, e + 12);
  offset += f.data.length;
});
writeFileSync(join(web, "public", "favicon.ico"), Buffer.concat([head, ...frames.map((f) => f.data)]));
console.log("public/favicon.ico");

await browser.close();
