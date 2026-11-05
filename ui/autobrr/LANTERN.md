# Lantern's changes to autobrr's frontend

This folder is autobrr's web frontend (the contents of `web/` of [autobrr/autobrr](https://github.com/autobrr/autobrr),
GPL-2.0-or-later, plus its `LICENSE`) at the commit named in `UPSTREAM`, brought in by `scripts/upstream-merge.sh`
(see `UPSTREAM.md` at the repository root). Everything else here is autobrr's, unchanged. Each edit in upstream's
files carries a `LANTERN:` comment; this list says why, so a conflict in a later merge is easy to judge. Lantern's
own files are in `src/lantern/`.

Build it as autobrr does: `corepack enable && pnpm install --frozen-lockfile && pnpm run build` (Node 24; pnpm at the
version `package.json` pins). The result is in `dist`, which Lantern serves at `/autobrr/` (`APP_AUTOBRR_UI_DIR`),
filling in the `{{.BaseUrl}}`, `{{.AssetBaseUrl}}` and `{{.InstanceName}}` the build leaves in `index.html` and
`manifest.webmanifest`, as autobrr's server does. The frontend's API calls go to `/autobrr/api/...`.

## The kit's Encore look

| File | Change |
| --- | --- |
| `src/lantern/kit-tokens.css` (new) | The kit's generated colour tokens (Krifmaz/kit v0.8.0 `design/out/tokens.css`), copied as they are (the same file as `ui/prowlarr`'s): Encore on `:root`, then `data-theme` `dark`, `oled` and `light`. Copy it again when the kit tag changes. |
| `src/lantern/colors.ts` (new) | Every Tailwind colour scale autobrr uses (gray and its zinc/slate/neutral/stone twins, blue, red, green, amber, …, each in steps of 25), drawn from the tokens: greys between the theme's page, panel, border and text colours; blue is the accent (fills) and the second accent (links, focus rings); red, green, amber and sky are `--err`, `--ok`, `--warn` and `--info`, tinted toward the theme's lightest and darkest colours. |
| `src/lantern/encore.css` (new) | The fixed grey steps and the lightest and darkest colours for autobrr's dark mode (`html.dark`) and light mode; Tailwind's white and black; `--on-accent` as "white" on blue fills (Encore's accent is light); the kit's monospace font. |
| `src/lantern/theme.ts` (new) | Keeps `<html data-theme>` in step with autobrr's `dark` class: dark mode is Encore, light mode the kit's light theme; the browser bar colour is the theme's page colour. |
| `src/lantern/chartColors.ts` (new) | The dashboard charts' colours, read from the tokens when a chart draws (the chart library needs real colours to mix). |
| `tailwind.config.ts` | Puts `src/lantern/colors.ts` over tailwind-lerp-colors' scales (autobrr's `gray` was zinc with an extra 815 step, kept). |
| `src/index.css` | Loads `kit-tokens.css` and `encore.css`. |
| `src/index.tsx` | Starts `followKitTheme()`. |
| `index.html` | The kit's fonts (the system's own, as kit-leptos' `kit.css`), not Inter; `data-theme="light"` before the first paint in light mode; the kit's Encore page colour for the browser bar. |
| `src/screens/dashboard/chartTheme.ts`, `HourHeatmap.tsx`, `TopLists.tsx` | The chart series (approved `--ok`, matches the second accent, rejected `--faint`, errored `--err`), the heatmap ramp, the chart text, grid and background, the empty heatmap cell and the top-list track come from `chartColors.ts` instead of Tailwind's fixed hues. |

Layouts, sizes, spacing and autobrr's background pattern stay as they are.

## Branding and links

| File | Change |
| --- | --- |
| `src/lantern/Wordmark.tsx` (new) | The instance name (Lantern's `app_name` setting, put into `index.html` by Lantern's server) as bold text, where autobrr's logo was. |
| `src/components/header/LeftNav.tsx` | The header shows the instance name instead of autobrr's logo; no Docs link to autobrr.com. |
| `src/screens/auth/Login.tsx`, `src/screens/auth/Onboarding.tsx` | The instance name instead of the logo and "autobrr" under it. Lantern signs people in on its own page, so these pages are rarely seen. |
| `src/components/alerts/NotFound.tsx`, `src/screens/filters/NotFound.tsx` | The instance name instead of the logo; no "report this to our GitHub or Discord" lines. |
| `src/components/alerts/ErrorPage.tsx` | No "report this to our GitHub or Discord" line (Lantern has its own bug reports). |
| `src/components/ExternalLink.tsx` | `DocsLink` draws nothing for autobrr.com addresses: the docs links beside filter and settings fields are gone, their help texts stay. Links to other sites stay. |
| `src/screens/Releases.tsx` | The search help has no link to autobrr's search docs. |
| `index.html`, `vite.config.ts` | Title, description and app manifest say Lantern, with the kit's Encore page colour; no autobrr icons (favicon, touch icons, manifest icons). |

autobrr's images (`public/`, `src/logo.svg`) stay in the tree but are not used by the pages.

## Not changed on purpose

- The update banner and the "… available!" chip in Settings → Application show what the API answers
  (`APP_UPDATE` alerts, `api/updates/latest`). Lantern's autobrr API never points them at autobrr's releases, so
  the frontend keeps them as they are (autobrr's own tests check the banner).
- The translations (`src/i18n/locales/`) are autobrr's. Some help texts name autobrr: several of them mean autobrr
  itself (its filter JSON format, its hosted lists at api.autobrr.com, Notifiarr's autobrr integration), so a
  word-for-word swap would make them wrong; they are left for a later change that rewords each one.
- Links to other projects' sites (Trakt, qBittorrent, Discord webhooks, …) in forms stay: they help set those up.
