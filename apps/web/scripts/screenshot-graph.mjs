import puppeteer from 'puppeteer';

const OUT = '../../docs/web-screens/06-graph-lab.png';

(async () => {
  const browser = await puppeteer.launch({
    headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox'],
  });
  const page = await browser.newPage();
  await page.setViewport({ width: 1480, height: 1100, deviceScaleFactor: 1 });
  await page.goto('http://127.0.0.1:3000/graph', { waitUntil: 'networkidle0', timeout: 30_000 });
  await new Promise(r => setTimeout(r, 1500));
  // Click "Run PPR"
  const buttons = await page.$$('button');
  for (const b of buttons) {
    const txt = await page.evaluate(el => el.textContent ?? '', b);
    if (txt.trim() === 'Run PPR') { await b.click(); break; }
  }
  await new Promise(r => setTimeout(r, 1500));
  await page.screenshot({ path: OUT, fullPage: false });
  console.log('shot', OUT);
  await browser.close();
})();
