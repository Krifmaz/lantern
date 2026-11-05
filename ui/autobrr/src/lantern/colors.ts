/*
 * LANTERN: the kit's Encore look for autobrr's frontend (Tailwind colours).
 *
 * autobrr draws everything with Tailwind's colour scales (gray, blue, red, green, ...), stretched by
 * tailwind-lerp-colors to steps of 25 (gray-815, blue-525, ...). tailwind.config.ts replaces those scales with the
 * ones below. Every step is a colour of the kit's generated tokens (src/lantern/kit-tokens.css: --bg, --panel,
 * --text, --accent, --ok, ...) or a color-mix() of two of them, so no colour is Lantern's own and each theme the
 * tokens have (picked by <html data-theme>) draws the whole frontend.
 *
 * The same class means a different thing in autobrr's dark and light modes (dark:bg-gray-800 is a card on a dark
 * page, text-gray-800 is text on a light one), so the scales go from the theme's lightest colour (step 0) to its
 * darkest (step 1000): src/lantern/encore.css sets those ends, and the grey steps, for html.dark and for the light
 * mode. The chart series colours stay autobrr's (src/screens/dashboard/chartTheme.ts): charts need hues the tokens
 * do not have.
 */

/** Every step tailwind-lerp-colors makes: 0, 25, 50, ... 1000. */
const STEPS = Array.from({ length: 41 }, (_, i) => i * 25);

/** `a` with `percent`% of it mixed into `b` (the kit's tokens are sRGB colours). */
const mix = (a: string, percent: number, b: string) =>
  `color-mix(in srgb, ${a} ${Math.round(percent * 10) / 10}%, ${b})`;

/** The theme's lightest and darkest colours (encore.css). */
const LIGHT = "var(--lantern-light)";
const DARK = "var(--lantern-dark)";

/**
 * A full scale from a few fixed steps: each step in between is a mix of the two fixed steps around it, in
 * proportion to how close it is to each.
 */
function scale(anchors: Array<[number, string]>): Record<string, string> {
  const out: Record<string, string> = {};
  for (const step of STEPS) {
    const upper = anchors.findIndex(([k]) => k >= step);
    const [k2, v2] = anchors[upper];
    if (k2 === step || upper === 0) {
      out[step] = v2;
      continue;
    }
    const [k1, v1] = anchors[upper - 1];
    out[step] = v1 === v2 ? v1 : mix(v1, ((k2 - step) / (k2 - k1)) * 100, v2);
  }
  return out;
}

/** The anchors of a tinted scale around `t`: pale tints of it below 500, `t` at 500, deep shades above. */
const tintAnchors = (t: string): Array<[number, string]> => [
  [0, LIGHT],
  [50, mix(t, 8, LIGHT)],
  [100, mix(t, 15, LIGHT)],
  [200, mix(t, 30, LIGHT)],
  [300, mix(t, 55, LIGHT)],
  [400, mix(t, 80, LIGHT)],
  [500, t],
  [600, mix(t, 85, DARK)],
  [700, mix(t, 70, DARK)],
  [800, mix(t, 55, DARK)],
  [900, mix(t, 35, DARK)],
  [950, mix(t, 22, DARK)],
  [1000, DARK]
];

/** A tinted scale around the token `--<token>`. */
const tint = (token: string) => scale(tintAnchors(`var(--${token})`));

/** The greys: the steps encore.css sets for each mode, the rest mixed between them. */
const gray = scale(
  [0, 50, 100, 200, 300, 400, 500, 600, 700, 750, 800, 850, 900, 950, 1000].map((k) => [k, `var(--lantern-gray-${k})`])
);
// autobrr's own extra step (its gray-815, between 800 and 825)
gray[815] = mix("var(--lantern-gray-800)", 70, "var(--lantern-gray-850)");

/**
 * autobrr's blue is the kit's accent: links, focus rings and outlines are the second accent (--accent-2, as the
 * kit's inputs and Prowlarr's fork use it), filled buttons (blue-600, blue-700) are the accent itself, with
 * --on-accent text (encore.css).
 */
const accent2Anchors = tintAnchors("var(--accent-2)");
const blue = scale([
  ...accent2Anchors.filter(([k]) => k < 500),
  [500, "var(--accent-2)"],
  [550, mix("var(--accent-2)", 50, "var(--accent)")],
  [600, "var(--accent)"],
  [700, mix("var(--accent)", 82, "var(--accent-2)")],
  ...accent2Anchors.filter(([k]) => k >= 800)
]);

const ok = tint("ok");
const warn = tint("warn");
const err = tint("err");
const info = tint("info");
const accent2 = tint("accent-2");

/** The colours tailwind.config.ts puts over tailwind-lerp-colors' scales. */
export const lanternColors = {
  // text on dark fills in dark mode, cards in light mode (encore.css)
  white: "var(--lantern-white)",
  black: "var(--lantern-black)",
  gray,
  zinc: gray,
  slate: gray,
  neutral: gray,
  stone: gray,
  blue,
  sky: info,
  cyan: info,
  red: err,
  rose: err,
  green: ok,
  emerald: ok,
  lime: ok,
  teal: ok,
  amber: warn,
  yellow: warn,
  orange: warn,
  purple: accent2,
  violet: accent2,
  fuchsia: accent2,
  indigo: accent2,
  pink: accent2
};
