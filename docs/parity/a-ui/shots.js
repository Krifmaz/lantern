// Screenshots of Lantern's autobrr pages (ui/autobrr) at desktop and phone width, in the kit's Encore look.
//
// Usage: NODE_PATH=<global node_modules> node shots.js <lantern URL> <out folder>
// Lantern must run with the built frontend (APP_AUTOBRR_UI_DIR=ui/autobrr/dist) and an owner admin /
// "correct horse battery" (APP_ADMIN_USER, APP_ADMIN_PASSWORD). The PNGs in this folder were then cut to a palette
// of 64 colours (Pillow's quantize, no dithering) to keep them small.
//
// The frontend needs API answers Lantern does not have yet (docs/PARITY.md, track A-api). Until they are ported,
// every /autobrr/api call Lantern does not answer with JSON gets a stand-in in the browser, shaped like autobrr's
// answer (web/src/types), with a few made-up filters, IRC networks and release counts so the pages draw as they do
// in use and their look can be checked. Answers Lantern has are used as they are; the live event streams are closed.
const { chromium } = require('playwright');

const [base, out] = process.argv.slice(2);

const day = 24 * 3600 * 1000;
const now = Date.now();
const iso = (t) => new Date(t).toISOString();
const days = (n, f) => Array.from({ length: n }, (_, i) => f(new Date(now - (n - 1 - i) * day).toISOString().slice(0, 10), i));
// a smooth made-up wave, the same every run
const wave = (i, lo, hi) => Math.round(lo + ((hi - lo) * (1 + Math.sin(i / 2.3))) / 2);

const filter = (id, name, enabled, priority, indexers, actions, enabledActions) => ({
  id, name, enabled, priority, created_at: iso(now - 90 * day), updated_at: iso(now - 3 * day),
  indexers: indexers.map((n, i) => ({ id: i + 1, name: n, identifier: n.toLowerCase(), identifier_external: n })),
  actions_count: actions, actions_enabled_count: enabledActions, is_auto_updated: false,
  max_downloads: 0, max_downloads_unit: '', max_downloads_period: 0, max_downloads_window_type: '', downloads: null
});

const channel = (id, name, state, lastAnnounce) => ({
  id, enabled: true, name, password: '', detached: false, monitoring: state === 'Monitoring', state,
  monitoring_since: iso(now - 2 * day), last_announce: lastAnnounce, connection_errors: []
});

const network = (id, name, server, connected, channels) => ({
  id, name, enabled: true, server, port: 6697, tls: true, tls_skip_verify: false, nick: 'lantern_bot', pass: '',
  auth: { mechanism: 'SASL_PLAIN', account: 'lantern_bot' }, invite_command: '', use_bouncer: false, bouncer_addr: '',
  bot_mode: false, channels, connected, connected_since: iso(now - 2 * day), use_proxy: false, proxy_id: 0,
  connection_errors: connected ? [] : ['dial tcp: connection refused'], healthy: connected
});

const release = (id, name, indexer, filterName, status, minutes) => ({
  id, filter_status: 'FILTER_APPROVED', rejections: [], indexer: { id: 1, name: indexer, identifier: indexer.toLowerCase(), identifier_external: indexer },
  filter: filterName, protocol: 'torrent', implementation: 'IRC', announce_type: 'NEW', name, title: name, size: 4_831_838_208,
  raw: '', info_url: '', download_url: '', category: 'TV', group: '', season: 0, episode: 0, year: 0, resolution: '1080p',
  codec: 'H.264', source: 'WEB-DL', container: 'mkv', hdr: '', uploader: '', origin: '', timestamp: iso(now - minutes * 60_000),
  action_status: [{ id, status: status, action: 'qBittorrent', action_id: 1, type: 'QBITTORRENT', client: 'qBittorrent',
    filter: filterName, filter_id: 1, release_id: id, rejections: [], timestamp: iso(now - minutes * 60_000) }]
});

const filters = [
  filter(1, 'TV 1080p WEB', true, 10, ['TorrentLeech', 'IPTorrents'], 2, 2),
  filter(2, 'Movies 2160p remux', true, 5, ['BeyondHD'], 1, 1),
  filter(3, 'Music FLAC', false, 0, ['Orpheus', 'Redacted'], 1, 0),
  filter(4, 'Anime batches', true, 0, ['Nyaa'], 2, 1)
];

const stubs = {
  'auth/validate': { username: 'admin', auth_method: 'password' },
  alerts: [],
  filters,
  'indexer/options': ['TorrentLeech', 'IPTorrents', 'BeyondHD', 'Orpheus', 'Redacted', 'Nyaa'].map((n) => ({ label: n, value: n.toLowerCase(), identifier: n.toLowerCase() })),
  irc: [
    network(1, 'TorrentLeech', 'irc.torrentleech.org', true, [channel(1, '#tlannounces', 'Monitoring', iso(now - 4 * 60_000))]),
    network(2, 'IPTorrents', 'irc.iptorrents.com', true, [channel(2, '#ipt.announce', 'Monitoring', iso(now - 11 * 60_000)), channel(3, '#ipt.announce2', 'Monitoring', iso(now - 32 * 60_000))]),
    network(3, 'BeyondHD', 'irc.beyond-hd.me', false, [channel(4, '#bhd_announce', 'Joining', '')])
  ],
  'notification/inbox': { data: [], count: 0, all_count: 0, unread_count: 0 },
  'release/recent': {
    data: [
      release(1, 'Some.Show.S02E05.1080p.WEB-DL.DDP5.1.H.264-GROUP', 'TorrentLeech', 'TV 1080p WEB', 'PUSH_APPROVED', 7),
      release(2, 'Another.Show.S01E01.1080p.WEB.h264-TEAM', 'IPTorrents', 'TV 1080p WEB', 'PUSH_APPROVED', 52),
      release(3, 'A.Movie.2024.2160p.UHD.BluRay.REMUX.HDR.HEVC-RG', 'BeyondHD', 'Movies 2160p remux', 'PUSH_REJECTED', 180)
    ],
    next_cursor: 0, count: 3
  },
  'release/stats': { total_count: 18421, filtered_count: 812, filter_rejected_count: 17609, push_approved_count: 655, push_rejected_count: 142, push_error_count: 15 },
  'release/stats/activity': { days: 30, daily: days(30, (date, i) => ({ date, matched_count: wave(i, 18, 40), push_approved_count: wave(i, 12, 30), push_rejected_count: wave(i + 3, 2, 9), push_error_count: i % 9 === 0 ? 2 : 0 })) },
  'release/stats/volume': { days: 30, daily: days(30, (date, i) => ({ date, downloaded_bytes: wave(i, 20, 90) * 1_073_741_824 })) },
  'release/stats/heatmap': { days: 30, heatmap: Array.from({ length: 168 }, (_, i) => wave(i % 24, 0, 12) + ((i / 24) | 0) % 3) },
  'release/stats/indexers': { days: 30, top: [['TorrentLeech', 402, 351], ['IPTorrents', 251, 198], ['BeyondHD', 97, 71], ['Nyaa', 62, 35]].map(([indexer, matched_count, push_approved_count]) => ({ indexer, matched_count, push_approved_count })) },
  'release/stats/filters': { days: 30, top: [['TV 1080p WEB', 512, 430], ['Movies 2160p remux', 97, 71], ['Anime batches', 62, 35]].map(([filter, matched_count, push_approved_count]) => ({ filter, matched_count, push_approved_count })) }
};

const pages = [
  ['dashboard', '/autobrr/'],
  ['filters', '/autobrr/filters'],
  ['settings-irc', '/autobrr/settings/irc']
];
const desktop = { viewport: { width: 1280, height: 800 }, deviceScaleFactor: 1 };
const phone = { viewport: { width: 390, height: 844 }, deviceScaleFactor: 1, isMobile: true, hasTouch: true };
// [suffix, browser options, system colour scheme, pages]: autobrr's "system" theme is dark mode (Encore) on a dark
// system and light mode (the kit's light theme) on a light one
const sizes = [
  ['desktop', desktop, 'dark', pages],
  ['phone', phone, 'dark', pages],
  ['desktop-light', desktop, 'light', pages.slice(0, 1)]
];

(async () => {
  const browser = await chromium.launch();
  for (const [size, options, colorScheme, list] of sizes) {
    const context = await browser.newContext({ ...options, colorScheme });
    const login = await context.request.post(`${base}/api/auth/login`, {
      data: { username: 'admin', password: 'correct horse battery' }
    });
    if (!login.ok()) throw new Error(`sign-in failed: ${login.status()}`);
    await context.route('**/autobrr/api/**', async (route) => {
      const url = new URL(route.request().url());
      const path = url.pathname.replace(/^\/autobrr\/api\//, '');
      if (path === 'events' || path === 'irc/events') {
        // the live streams: 204 tells the browser not to reconnect
        return route.fulfill({ status: 204 });
      }
      const response = await route.fetch();
      const type = response.headers()['content-type'] || '';
      if (response.ok() && type.includes('json')) {
        return route.fulfill({ response });
      }
      const stub = stubs[path];
      if (stub === undefined) {
        console.log(`[${size}] no answer for ${path} (${response.status()})`);
        return route.fulfill({ response });
      }
      return route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(stub) });
    });
    const page = await context.newPage();
    page.on('pageerror', (e) => console.log(`[${size}] page error: ${e.message}`));
    for (const [name, path] of list) {
      await page.goto(base + path, { waitUntil: 'networkidle' });
      await page.waitForTimeout(1500);
      const file = `${out}/${name}-${size}.png`;
      await page.screenshot({ path: file });
      console.log(file);
    }
    if (size === 'phone') {
      // the menu is folded away on a phone and opens from the header's menu button
      await page.goto(`${base}/autobrr/`, { waitUntil: 'networkidle' });
      await page.getByRole('button', { name: /open main menu/i }).click();
      await page.waitForTimeout(800);
      await page.screenshot({ path: `${out}/menu-open-phone.png` });
    }
    await context.close();
  }
  await browser.close();
})();
