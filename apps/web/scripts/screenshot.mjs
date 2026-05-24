// Capture screenshots of every route.  Assumes server (3030) and
// Next.js (3000) are already running.

import puppeteer from 'puppeteer';
import { mkdir } from 'node:fs/promises';
import { dirname } from 'node:path';

const OUT_DIR = process.argv[2] ?? '../../docs/web-screens';

async function shoot(page, path, file) {
  await page.goto(`http://127.0.0.1:3000${path}`, { waitUntil: 'networkidle0', timeout: 30_000 });
  // Let the page settle so probe traffic populates the dashboard.
  await new Promise(r => setTimeout(r, path === '/' ? 5000 : 1500));
  await page.screenshot({ path: file, fullPage: false });
  console.log('shot', file);
}

async function shootWithAction(page, path, file, action) {
  await page.goto(`http://127.0.0.1:3000${path}`, { waitUntil: 'networkidle0' });
  await new Promise(r => setTimeout(r, 1000));
  await action(page);
  await new Promise(r => setTimeout(r, 1500));
  await page.screenshot({ path: file, fullPage: false });
  console.log('shot', file, '(with action)');
}

(async () => {
  await mkdir(OUT_DIR, { recursive: true });
  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1480, height: 920, deviceScaleFactor: 1 });

  // Operations dashboard — wait for probe traffic.
  await shoot(page, '/', `${OUT_DIR}/01-operations.png`);

  // Query Lab with results.
  await shootWithAction(page, '/query-lab', `${OUT_DIR}/02-query-lab.png`, async (page) => {
    await page.click('button[class*="bg-accent-500"]'); // "Run hybrid search"
  });

  // Tool Explorer.
  await shoot(page, '/tools', `${OUT_DIR}/03-tools.png`);

  // Skill Lab — pick a skill and fill arguments.
  await shootWithAction(page, '/skills', `${OUT_DIR}/04-skills.png`, async (page) => {
    // Type into the company_code input
    const inputs = await page.$$('input[type="text"]');
    if (inputs.length >= 1) {
      await inputs[0].click({ clickCount: 3 });
      await inputs[0].type('1000');
    }
    if (inputs.length >= 2) {
      await inputs[1].click({ clickCount: 3 });
      await inputs[1].type('2026-M03');
    }
  });

  // Resources.
  await shootWithAction(page, '/resources', `${OUT_DIR}/05-resources.png`, async (page) => {
    // Click first sap-rfc resource
    const buttons = await page.$$('button');
    for (const b of buttons) {
      const txt = await page.evaluate(el => el.textContent ?? '', b);
      if (txt.includes('BAPI_ACC_DOCUMENT_POST')) { await b.click(); break; }
    }
  });

  await browser.close();
})();
