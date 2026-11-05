// Screenshots of Lantern's Prowlarr pages (ui/prowlarr) and of Lantern's kit look, at desktop and phone width.
//
// Usage: NODE_PATH=<global node_modules> node shots.js <lantern URL> <out folder> <Prowlarr en.json>
// Lantern must run with the built frontend (APP_PROWLARR_UI_DIR) and an owner admin / "correct horse battery"
// (APP_ADMIN_USER, APP_ADMIN_PASSWORD). The kit reference page is kit-reference.html next to this file: put it and
// web/static/css/ (as css/) in the folder APP_STATIC_DIR names; it draws Lantern's shell with Lantern's own
// stylesheets (the kit's tokens.css and kit.css, then app.css).
// en.json is Prowlarr's src/NzbDrone.Core/Localization/Core/en.json at the UPSTREAM.lock commit.
//
// The frontend needs API answers Lantern does not have yet (docs/PARITY.md, track P-api). Until they are ported,
// every /api/v1 call Lantern does not answer with JSON gets a stand-in in the browser, shaped like Prowlarr's
// answer with Prowlarr's defaults (English strings from Prowlarr's en.json, empty lists, the default host and UI
// settings), so the pages draw and their look can be checked. Answers Lantern has are used as they are.
const fs = require('fs');
const { chromium } = require('playwright');

const [base, out, enJson] = process.argv.slice(2);
const strings = JSON.parse(fs.readFileSync(enJson, 'utf8'));

const stubs = {
  localization: { Strings: strings },
  customfilter: [],
  'config/ui': {
    id: 1, firstDayOfWeek: 0, calendarWeekColumnHeader: 'ddd M/D', shortDateFormat: 'MMM D YYYY',
    longDateFormat: 'dddd, MMMM D YYYY', timeFormat: 'h(:mm)a', showRelativeDates: true,
    enableColorImpairedMode: false, uiLanguage: 1, theme: 'auto'
  },
  'config/host': {
    id: 1, bindAddress: '*', port: 9696, sslPort: 6969, enableSsl: false, launchBrowser: true,
    authenticationMethod: 'forms', authenticationRequired: 'enabled', allowedHosts: '', analyticsEnabled: false,
    username: 'admin', password: '', passwordConfirmation: '', logLevel: 'info', logSizeLimit: 1,
    consoleLogLevel: '', branch: 'main', apiKey: '0123456789abcdef0123456789abcdef', sslCertPath: '',
    sslCertPassword: '', urlBase: '/prowlarr', trustedNetworks: '', instanceName: 'Lantern', applicationUrl: '',
    updateAutomatically: false, updateMechanism: 'docker', updateScriptPath: '', proxyEnabled: false,
    proxyType: 'http', proxyHostname: '', proxyPort: 8080, proxyUsername: '', proxyPassword: '',
    proxyBypassFilter: '', proxyBypassLocalAddresses: true, certificateValidation: 'enabled', backupFolder: 'Backups',
    backupInterval: 7, backupRetention: 28, historyCleanupDays: 365, trustCgnatIpAddresses: false
  },
  appprofile: [{ id: 1, name: 'Standard', enableRss: true, enableAutomaticSearch: true, enableInteractiveSearch: true, minimumSeeders: 1 }],
  indexerstatus: [],
  'indexer/categories': [],
  language: [{ id: 1, name: 'English' }],
  update: [],
  'system/task': [],
  command: [],
  indexerproxy: [],
  applications: [],
  downloadclient: [],
  notification: []
};

const pages = [
  ['indexers', '/prowlarr/'],
  ['settings-general', '/prowlarr/settings/general'],
  ['system-status', '/prowlarr/system/status'],
  ['kit-reference', '/kit-reference.html']
];
const desktop = { viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 };
const phone = { viewport: { width: 390, height: 844 }, deviceScaleFactor: 1, isMobile: true, hasTouch: true };
// [suffix, browser options, system colour scheme]: Prowlarr's "auto" theme draws Encore on a dark system and the
// kit's light theme on a light one
const sizes = [
  ['desktop', desktop, 'dark'],
  ['phone', phone, 'dark'],
  ['desktop-light', desktop, 'light']
];

(async () => {
  const browser = await chromium.launch();
  for (const [size, options, colorScheme] of sizes) {
    const context = await browser.newContext({ ...options, colorScheme });
    const login = await context.request.post(`${base}/api/auth/login`, {
      data: { username: 'admin', password: 'correct horse battery' }
    });
    if (!login.ok()) throw new Error(`sign-in failed: ${login.status()}`);
    await context.route('**/api/v1/**', async (route) => {
      const response = await route.fetch();
      const type = response.headers()['content-type'] || '';
      if (response.ok() && type.includes('json')) {
        return route.fulfill({ response });
      }
      const path = new URL(route.request().url()).pathname.replace(/^\/api\/v1\//, '');
      const stub = stubs[path.toLowerCase()];
      if (stub === undefined) {
        console.log(`[${size}] no answer for ${path} (${response.status()})`);
        return route.fulfill({ response });
      }
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(stub) });
    });
    const page = await context.newPage();
    page.on('pageerror', (e) => console.log(`[${size}] page error: ${e.message}`));
    for (const [name, path] of pages) {
      if (size === 'desktop-light' && name !== 'indexers') continue;
      await page.goto(base + path, { waitUntil: 'networkidle' });
      await page.waitForTimeout(1500);
      const file = `${out}/${name}-${size}.png`;
      await page.screenshot({ path: file });
      console.log(file);
    }
    if (size === 'phone') {
      // the sidebar is folded away on a phone and opens from the header's menu button
      await page.goto(`${base}/prowlarr/`, { waitUntil: 'networkidle' });
      await page.click('#sidebar-toggle-button');
      await page.waitForTimeout(800);
      await page.screenshot({ path: `${out}/sidebar-open-phone.png` });
    }
    await context.close();
  }
  await browser.close();
})();
