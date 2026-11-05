import { useCallback, useEffect } from 'react';
import { useSelector } from 'react-redux';
import { createSelector } from 'reselect';
import themes, { dataThemes } from 'Styles/Themes';
import AppState from './State/AppState';

function createThemeSelector() {
  return createSelector(
    (state: AppState) => state.settings.ui.item.theme || window.Prowlarr.theme,
    (theme) => {
      return theme;
    }
  );
}

function ApplyTheme() {
  const theme = useSelector(createThemeSelector());

  const updateCSSVariables = useCallback(() => {
    Object.entries(themes[theme]).forEach(([key, value]) => {
      document.documentElement.style.setProperty(`--${key}`, value);
    });

    // LANTERN: the kit's tokens follow <html data-theme> (none is Encore)
    const dataTheme = dataThemes[theme as keyof typeof dataThemes];

    if (dataTheme) {
      document.documentElement.dataset.theme = dataTheme;
    } else {
      delete document.documentElement.dataset.theme;
    }
  }, [theme]);

  // On Component Mount and Component Update
  useEffect(() => {
    updateCSSVariables();
  }, [updateCSSVariables, theme]);

  return null;
}

export default ApplyTheme;
