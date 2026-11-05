# Lantern's changes to Prowlarr's frontend

This folder is Prowlarr's web frontend (`frontend/` and the root build files of
[Prowlarr/Prowlarr](https://github.com/Prowlarr/Prowlarr), GPL-3.0, see `LICENSE`) at the commit named in
`UPSTREAM`, brought in by `scripts/upstream-merge.sh` (see `UPSTREAM.md` at the repository root). Everything else
here is Prowlarr's, unchanged. Each edit in upstream's files carries a `LANTERN:` comment; this list says why, so a
conflict in a later merge is easy to judge.

Build it as Prowlarr does: `yarn install --frozen-lockfile && yarn build --env production` (Node 22); the result is
in `_output/UI`, which Lantern serves at `/prowlarr/` (`APP_PROWLARR_UI_DIR`).

## The kit's Encore look

| File | Change |
| --- | --- |
| `frontend/src/Styles/Themes/kit-tokens.css` (new) | The kit's generated colour tokens (Krifmaz/kit v0.8.0 `design/out/tokens.css`), copied as they are: Encore on `:root`, then `data-theme` `dark`, `oled` and `light`. Copy it again when the kit tag changes. |
| `frontend/src/Styles/Themes/encore.js` (new) | Every variable of Prowlarr's `dark.js` and `light.js`, mapped to the tokens (`var(--panel)`, …; tints with `color-mix()`), plus `onAccentColor` for text on the accent. The chart palettes stay Prowlarr's. |
| `frontend/src/Styles/Themes/index.js` | The themes are `auto`, `encore`, `dark`, `oled` and `light`, all drawn by `encore.js`; `dataThemes` says which `data-theme` each sets. `auto` follows the system: Encore when it is dark, the light theme when not. |
| `frontend/src/App/ApplyTheme.tsx` | Also sets `<html data-theme>` for the chosen theme. |
| `frontend/src/Styles/globals.css` | Loads `kit-tokens.css`. |
| `frontend/src/Styles/Variables/fonts.js`, `frontend/src/Styles/scaffolding.css` | The kit's fonts: the system's own (`system-ui`, …, `ui-monospace`), as kit-leptos' `kit.css`. Sizes are Prowlarr's. |
| `frontend/src/Components/Link/Button.css`, `Components/Label.css`, `Components/Form/CheckInput.css` | Text on an accent fill is `--onAccentColor` (the kit's `--on-accent`), as the accent can be light; an empty check box is drawn like the other inputs. |
| `Settings/Settings.css`, `Components/FieldSet.css`, `Components/Page/Toolbar/PageToolbarSeparator.css`, `System/Status/styles.css`, `System/Updates/Updates.css`, `App/AppUpdatedModalContent.css` | Rules that were a fixed light grey (`#e5e5e5`) use `--borderColor`. |
| `Components/Page/Header/PageHeader.css`, `PageHeaderActionsMenu.css` | Header links use the header's text colour and the accent on hover instead of fixed greys. |
| `.stylelintignore` (new) | Prowlarr's style rules skip the copied kit tokens. |

## Branding and links

| File | Change |
| --- | --- |
| `Components/Page/Header/PageHeader.js`, `.css`, `.css.d.ts` | The header shows the instance name (Lantern) as Lantern's own pages do, in place of Prowlarr's logo; no donate or translate links. |
| `System/Status/Status.tsx` | No "More info" (Prowlarr's home page, wiki, Reddit, Discord, GitHub) and no donations. |
| `System/Status/Health/Health.tsx`, `Store/Selectors/createHealthCheckSelector.js` | A wiki button only when a health item has a link; none to Prowlarr's wiki. |
| `Settings/General/HostSettings.js`, `SecuritySettings.js`, `UpdateSettings.js`, `FirstRun/AuthenticationRequiredModalContent.tsx`, `Settings/Notifications/Notifications/NotificationEventItems.js`, `Components/FileBrowser/FileBrowserModalContent.js` | No help links to Prowlarr's wiki. |
| `System/Updates/UpdateChanges.tsx` | Issue numbers in change lists stay text, with no links to Prowlarr's GitHub. |
| `Settings/General/GeneralSettings.js` | No Analytics section. |
| `Store/Middleware/createSentryMiddleware.js` | Errors never go to Servarr's Sentry; Lantern keeps its own crash logs. |
| `Utilities/String/translate.ts` | `{appName}` in Prowlarr's texts is Lantern. |
| `frontend/src/index.ejs`, `frontend/src/Content/manifest.json` | Title, description and browser colours are Lantern's (the kit's Encore background); no Prowlarr icons. |

Prowlarr's images (`Content/Images`) and its `login.html` stay in the tree but are not used: Lantern signs people in
on its own page.
