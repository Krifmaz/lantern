// LANTERN: the kit's Encore look for Prowlarr's frontend.
//
// Every variable Prowlarr's themes (dark.js, light.js) define, mapped to the kit's generated colour tokens
// (kit-tokens.css: --bg, --panel, --text, --accent, --ok, ...). The tokens change with <html data-theme>
// (none = Encore, "dark", "oled", "light"), so this one map draws every theme; ApplyTheme sets the attribute.
// Tints and shades are mixed from the tokens with color-mix(), so no colour is Lantern's own. The chart
// palettes are Prowlarr's, as charts need many distinct colours that the tokens do not have.

const mix = (token, percent, other = 'transparent') =>
  `color-mix(in srgb, var(--${token}) ${percent}%, ${other})`;

module.exports = {
  textColor: 'var(--text)',
  defaultColor: 'var(--muted)',
  disabledColor: 'var(--faint)',
  dimColor: 'var(--faint)',
  black: 'var(--bg)',
  // text on dark fills and on the header: the theme's text colour
  white: 'var(--text)',
  primaryColor: 'var(--accent)',
  selectedColor: 'var(--accent-2)',
  successColor: 'var(--ok)',
  dangerColor: 'var(--err)',
  warningColor: 'var(--warn)',
  infoColor: 'var(--info)',
  purple: 'var(--accent-2)',
  prowlarrOrange: 'var(--accent)',
  helpTextColor: 'var(--muted)',
  darkGray: 'var(--muted)',
  gray: 'var(--faint)',

  // Text on a fill of the accent colour (primary buttons, default labels, checked boxes)
  onAccentColor: 'var(--on-accent)',

  // Theme Colors

  themeBlue: 'var(--accent)',
  themeRed: 'var(--err)',
  themeDarkColor: 'var(--panel-2)',
  themeLightColor: 'var(--accent)',
  pageBackground: 'var(--bg)',
  pageFooterBackgroud: 'var(--bg-2)',

  torrentColor: 'var(--ok)',
  usenetColor: 'var(--info)',

  // Labels
  inverseLabelColor: 'var(--panel-2)',
  inverseLabelTextColor: 'var(--accent)',

  // Links
  defaultLinkHoverColor: 'var(--text)',
  linkColor: 'var(--accent-2)',
  linkHoverColor: 'var(--accent)',

  // Header
  pageHeaderBackgroundColor: 'var(--bg-2)',

  // Sidebar

  sidebarColor: 'var(--text)',
  sidebarBackgroundColor: 'var(--bg-2)',
  sidebarActiveBackgroundColor: 'var(--panel)',

  // Toolbar
  toolbarColor: 'var(--text)',
  toolbarBackgroundColor: 'var(--bg-2)',
  toolbarMenuItemBackgroundColor: 'var(--panel)',
  toolbarMenuItemHoverBackgroundColor: 'var(--panel-2)',
  toolbarLabelColor: 'var(--muted)',

  // Accents
  borderColor: 'var(--border)',
  inputBorderColor: 'var(--border)',
  inputBoxShadowColor: 'transparent',
  inputFocusBorderColor: 'var(--accent-2)',
  inputFocusBoxShadowColor: 'rgba(var(--accent-2-rgb), 0.35)',
  inputErrorBorderColor: 'var(--err)',
  inputErrorBoxShadowColor: mix('err', 40),
  inputWarningBorderColor: 'var(--warn)',
  inputWarningBoxShadowColor: mix('warn', 40),
  colorImpairedGradient: 'var(--panel)',
  colorImpairedGradientDark: 'var(--panel-2)',

  //
  // Buttons

  defaultButtonTextColor: 'var(--text)',
  defaultButtonBackgroundColor: 'var(--panel-2)',
  defaultBorderColor: 'var(--border)',
  defaultHoverBackgroundColor: 'var(--border)',
  defaultHoverBorderColor: 'var(--faint)',

  primaryBackgroundColor: 'var(--accent)',
  primaryBorderColor: 'var(--accent)',
  primaryHoverBackgroundColor: 'var(--accent-2)',
  primaryHoverBorderColor: 'var(--accent-2)',

  successBackgroundColor: 'var(--ok)',
  successBorderColor: 'var(--ok)',
  successHoverBackgroundColor: mix('ok', 85, 'var(--bg)'),
  successHoverBorderColor: mix('ok', 85, 'var(--bg)'),

  warningBackgroundColor: 'var(--warn)',
  warningBorderColor: 'var(--warn)',
  warningHoverBackgroundColor: mix('warn', 85, 'var(--bg)'),
  warningHoverBorderColor: mix('warn', 85, 'var(--bg)'),

  dangerBackgroundColor: 'var(--err)',
  dangerBorderColor: 'var(--err)',
  dangerHoverBackgroundColor: mix('err', 85, 'var(--bg)'),
  dangerHoverBorderColor: mix('err', 85, 'var(--bg)'),

  iconButtonDisabledColor: 'var(--faint)',
  iconButtonHoverColor: 'var(--muted)',
  iconButtonHoverLightColor: 'var(--text)',

  //
  // Modal

  modalBackdropBackgroundColor: mix('bg', 70),
  modalBackgroundColor: 'var(--panel)',
  modalCloseButtonHoverColor: 'var(--muted)',

  //
  // Menu
  menuItemColor: 'var(--text)',
  menuItemHoverColor: 'var(--accent)',
  menuItemHoverBackgroundColor: 'var(--panel-2)',

  //
  // Toolbar

  toobarButtonHoverColor: 'var(--accent)',
  toobarButtonSelectedColor: 'var(--accent)',

  //
  // Scroller

  scrollbarBackgroundColor: 'var(--border)',
  scrollbarHoverBackgroundColor: 'var(--faint)',

  //
  // Card

  cardBackgroundColor: 'var(--panel)',
  cardShadowColor: 'var(--bg)',
  cardAlternateBackgroundColor: 'var(--panel)',
  cardCenterBackgroundColor: 'var(--bg-2)',

  //
  // Alert

  alertDangerBorderColor: 'var(--err)',
  alertDangerBackgroundColor: mix('err', 12),
  alertDangerColor: 'var(--text)',

  alertInfoBorderColor: 'var(--info)',
  alertInfoBackgroundColor: mix('info', 12),
  alertInfoColor: 'var(--text)',

  alertSuccessBorderColor: 'var(--ok)',
  alertSuccessBackgroundColor: mix('ok', 12),
  alertSuccessColor: 'var(--text)',

  alertWarningBorderColor: 'var(--warn)',
  alertWarningBackgroundColor: mix('warn', 12),
  alertWarningColor: 'var(--text)',

  //
  // Form

  inputBackgroundColor: 'var(--bg-2)',
  inputReadOnlyBackgroundColor: 'var(--panel)',
  inputHoverBackgroundColor: mix('text', 20),
  inputSelectedBackgroundColor: mix('text', 5),
  advancedFormLabelColor: 'var(--warn)',
  disabledCheckInputColor: 'var(--faint)',
  disabledInputColor: 'var(--faint)',

  //
  // Popover

  popoverTitleBackgroundColor: 'var(--panel-2)',
  popoverTitleBorderColor: 'var(--border)',
  popoverBodyBackgroundColor: 'var(--panel)',
  popoverShadowColor: 'rgba(0, 0, 0, 0.2)',
  popoverArrowBorderColor: 'var(--panel)',

  popoverTitleBackgroundInverseColor: 'var(--panel-2)',
  popoverTitleBorderInverseColor: 'var(--border)',
  popoverShadowInverseColor: 'rgba(0, 0, 0, 0.2)',
  popoverArrowBorderInverseColor: 'var(--panel-2)',

  //
  // Table

  tableRowHoverBackgroundColor: mix('text', 6),

  //
  // Charts

  chartBackgroundColor: 'var(--panel)',
  failedColors: ['#ffbeb2', '#feb4a6', '#fdab9b', '#fca290', '#fb9984', '#fa8f79', '#f9856e', '#f77b66', '#f5715d', '#f36754', '#f05c4d', '#ec5049', '#e74545', '#e13b42', '#da323f', '#d3293d', '#ca223c', '#c11a3b', '#b8163a', '#ae123a'].join(','),
  chartColorsDiversified: ['#90caf9', '#f4d166', '#ff8a65', '#ce93d8', '#80cba9', '#ffab91', '#8097ea', '#bcaaa4', '#a57583', '#e4e498', '#9e96af', '#c6ab81', '#6972c6', '#619fc6', '#81ad81', '#f48fb1', '#82afca', '#b5b071', '#8b959b', '#7ec0b4'].join(','),
  chartColors: ['#f4d166', '#f6c760', '#f8bc58', '#f8b252', '#f7a84a', '#f69e41', '#f49538', '#f38b2f', '#f28026', '#f0751e', '#eb6c1c', '#e4641e', '#de5d1f', '#d75521', '#cf4f22', '#c64a22', '#bc4623', '#b24223', '#a83e24', '#9e3a26'].join(',')
};
