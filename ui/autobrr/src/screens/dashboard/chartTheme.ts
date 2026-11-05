/*
 * Copyright (c) 2021 - 2025, Ludvig Lundgren and the autobrr contributors.
 * SPDX-License-Identifier: GPL-2.0-or-later
 */

import { SettingsContext } from "@utils/Context";
import { useMedia } from "@hooks/hooks";
// LANTERN: the charts' colours come from the kit's tokens (src/lantern/chartColors.ts)
import { chartColors } from "@app/lantern/chartColors";

export const useIsDark = (): boolean => {
  const theme = SettingsContext.useSelector((s) => s.theme);
  const prefersDark = useMedia("(prefers-color-scheme: dark)");
  return theme === "dark" || (theme === "system" && prefersDark);
};

// Tailwind hues the app already uses (green-600, blue-500, zinc, red),
// validated for CVD separation and surface contrast against the card
// surfaces (#ffffff / #27272a). Rejected is deliberately gray: it is
// context next to the series that matter, and a warm hue beside the green
// fails colorblind separation. Keep the series order approved, matches,
// rejected, errored when they share a chart.
// LANTERN: the series, the ramp and the chart's own colours are the kit's tokens, in place of Tailwind's hues
// (approved --ok, matches the second accent, rejected --faint, errored --err); the series order stays.
export const seriesColors = (dark: boolean) => chartColors(dark).series;

export const sequentialRamp = (dark: boolean): string[] => chartColors(dark).ramp;

export const chartTheme = (dark: boolean) => {
  const c = chartColors(dark);
  return {
    foreground: c.foreground,
    muted: c.muted,
    grid: c.grid,
    background: c.background,
    palette: Object.values(c.series)
  };
};
