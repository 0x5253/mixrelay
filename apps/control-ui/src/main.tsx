import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';

import { App } from './app/App';
import { localPorts } from './app/localPorts';
import { browserLanguages } from './core/adapters/browser/languages';
import { browserThemePreferenceStorage } from './core/adapters/browser/themePreferenceStorage';
import { I18nProvider, i18n, resolveLocale } from './core/i18n';
import { NativeMenuLocale } from './core/i18n/NativeMenuLocale';
import { NativeShellContext } from './core/shell/NativeShellContext';
import { MixRelayProvider } from './core/theme/MixRelayProvider';
import { NativeWindowTheme } from './core/theme/NativeWindowTheme';

// Resources are bundled, so the language is in place before the first render.
void i18n.changeLanguage(resolveLocale(browserLanguages(navigator)));

const root = document.getElementById('root');

if (!root) {
  throw new Error('MixRelay root element is missing');
}

createRoot(root).render(
  <StrictMode>
    <NativeShellContext.Provider value={localPorts().nativeShell}>
      <MixRelayProvider store={browserThemePreferenceStorage()}>
        <NativeWindowTheme />
        <I18nProvider>
          <NativeMenuLocale />
          <App />
        </I18nProvider>
      </MixRelayProvider>
    </NativeShellContext.Provider>
  </StrictMode>,
);
