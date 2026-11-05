/*
 * LANTERN: autobrr's dark and light modes, drawn by the kit's themes.
 *
 * autobrr switches modes with the "dark" class on <html> (its Theme setting: dark, light or system). The kit's
 * tokens follow <html data-theme> instead: none is Encore, "light" is the light theme. This keeps the two in step:
 * dark mode is Encore, light mode the kit's light theme. It also gives the browser's own bars (meta theme-color)
 * the theme's page colour, where autobrr set its own greys.
 */

/** Sets <html data-theme> and the browser bar colour for the mode the "dark" class says. */
export function syncKitTheme(): void {
  const root = document.documentElement;
  const dark = root.classList.contains("dark");
  if (dark) {
    if (root.dataset.theme !== undefined) delete root.dataset.theme;
  } else if (root.dataset.theme !== "light") {
    root.dataset.theme = "light";
  }
  const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
  const bg = getComputedStyle(root).getPropertyValue("--bg").trim();
  if (meta && bg && meta.content !== bg) {
    meta.content = bg;
  }
}

/** Follows every change of the "dark" class from now on (autobrr's settings and the system's preference). */
export function followKitTheme(): void {
  syncKitTheme();
  new MutationObserver(syncKitTheme).observe(document.documentElement, { attributes: true, attributeFilter: ["class"] });
  // autobrr sets theme-color after the class; set ours after it
  const meta = document.querySelector('meta[name="theme-color"]');
  if (meta) {
    new MutationObserver(syncKitTheme).observe(meta, { attributes: true, attributeFilter: ["content"] });
  }
}
