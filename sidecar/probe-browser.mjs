import { chromium } from "playwright";

const tries = [
  { name: "system-chrome", opts: { channel: "chrome", headless: true } },
  { name: "bundled-chromium", opts: { headless: true } },
];

for (const t of tries) {
  try {
    const browser = await chromium.launch(t.opts);
    const page = await browser.newPage();
    await page.goto("about:blank");
    console.log(`OK ${t.name}  version=${browser.version()}`);
    await browser.close();
  } catch (e) {
    console.log(`FAIL ${t.name}: ${String(e).split("\n")[0]}`);
  }
}
