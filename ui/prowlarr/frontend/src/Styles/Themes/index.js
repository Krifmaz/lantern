// LANTERN: Prowlarr's theme names, all drawn by the kit's Encore tokens (encore.js). dataThemes says which
// <html data-theme> each one sets; "auto" follows the system: Encore when it is dark, the light theme when not.
import * as encore from './encore';

const defaultDark = window.matchMedia('(prefers-color-scheme: dark)').matches;

export const dataThemes = {
  auto: defaultDark ? null : 'light',
  encore: null,
  dark: 'dark',
  oled: 'oled',
  light: 'light'
};

export default {
  auto: encore,
  encore,
  dark: encore,
  oled: encore,
  light: encore
};
