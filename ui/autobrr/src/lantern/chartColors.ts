/*
 * LANTERN: the dashboard's chart colours from the kit's tokens.
 *
 * autobrr's charts take plain colours in JavaScript (src/screens/dashboard/chartTheme.ts): the chart library mixes
 * them for its colour scales, so it needs real colours, not CSS variables. These are read from the kit's tokens
 * (kit-tokens.css) on <html> when a chart draws, and mixed here as color-mix() would mix them. The dashboard draws
 * its charts again when the mode changes (useIsDark), after src/lantern/theme.ts has switched the tokens.
 */

/** A token's colour as `[r, g, b]`, or `fallback` when the page has no such token (tests). */
function rgb(name: string, fallback: string): [number, number, number] {
  const value = getComputedStyle(document.documentElement).getPropertyValue(`--${name}`).trim() || fallback;
  const hex = value.replace("#", "");
  const full = hex.length === 3 ? hex.split("").map((c) => c + c).join("") : hex.slice(0, 6);
  const n = parseInt(full, 16);
  return Number.isNaN(n) ? [128, 128, 128] : [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

/** `#rrggbb` of `a` with `percent`% of it mixed into `b`. */
function mix(a: [number, number, number], percent: number, b: [number, number, number]): string {
  const p = percent / 100;
  return "#" + a.map((v, i) => Math.round(v * p + b[i] * (1 - p)).toString(16).padStart(2, "0")).join("");
}

/** A token as `#rrggbb`. */
const color = (name: string, fallback: string) => mix(rgb(name, fallback), 100, [0, 0, 0]);

/**
 * Every colour autobrr's charts use, from the theme in effect: the series (approved is --ok, matches the second
 * accent, rejected --faint, errored --err), the count ramp (pale to full second accent), the chart's text, grid and
 * background, the empty heatmap cell (`dark` picks the raised panel on a dark page, the page colour on a light one)
 * and the track behind the top-list bars.
 */
export function chartColors(dark: boolean) {
  const accent = rgb("accent-2", "#d6c49a");
  const panel = rgb("panel", "#222224");
  return {
    series: {
      approved: color("ok", "#22c55e"),
      matches: color("accent-2", "#d6c49a"),
      rejected: color("faint", "#77725f"),
      errored: color("err", "#ef4444")
    },
    ramp: [20, 40, 60, 80, 100].map((p) => mix(accent, p, panel)),
    foreground: color("text", "#f0ead6"),
    muted: color("muted", "#aaa493"),
    grid: color("border", "#353538"),
    background: color("panel", "#222224"),
    zeroFill: dark ? color("panel-2", "#2a2a2d") : color("bg", "#f6f6fb"),
    track: mix(accent, 25, panel)
  };
}
