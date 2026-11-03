import type { Preview } from '@storybook/react-vite';
import { useGlobals } from 'storybook/internal/preview-api';
import { themePreference } from '../src/core/domain/appearance';

import { I18nProvider, i18n } from '../src/core/i18n';
import { MixRelayProvider } from '../src/core/theme/MixRelayProvider';

const preview: Preview = {
  loaders: [
    async ({ globals }) => {
      await i18n.changeLanguage(globals.locale as string);
      return {};
    },
  ],
  decorators: [
    (Story, context) => {
      const [, updateGlobals] = useGlobals();
      const viewport =
        context.globals.viewport?.value ??
        context.parameters.viewport?.value ??
        context.parameters.viewport?.defaultViewport;
      const width = context.parameters.viewport?.options?.[viewport]?.styles?.width;
      return (
        <MixRelayProvider
          theme={themePreference(context.globals.theme)}
          onThemeChange={(theme) => updateGlobals({ theme })}
        >
          <I18nProvider>
            <div
              className={context.parameters.layout === 'fullscreen' ? undefined : 'storybook-frame'}
              style={width ? { width, maxWidth: '100%' } : undefined}
            >
              <Story />
            </div>
          </I18nProvider>
        </MixRelayProvider>
      );
    },
  ],
  globalTypes: {
    theme: {
      description: '화면 테마',
      toolbar: {
        icon: 'circlehollow',
        items: [
          { value: 'system', title: 'System' },
          { value: 'light', title: 'Light' },
          { value: 'dark', title: 'Dark' },
        ],
        dynamicTitle: true,
      },
    },
    locale: {
      description: '언어 · Pseudo는 긴 번역 확인',
      toolbar: {
        icon: 'globe',
        items: [
          { value: 'ko-KR', title: '한국어' },
          { value: 'en', title: 'English' },
          { value: 'ja', title: '日本語' },
          { value: 'zh-Hans', title: '简体中文' },
          { value: 'zh-Hant', title: '繁體中文' },
          { value: 'de', title: 'Deutsch' },
          { value: 'fr', title: 'Français' },
          { value: 'es', title: 'Español' },
          { value: 'pt', title: 'Português' },
          { value: 'it', title: 'Italiano' },
          { value: 'ru', title: 'Русский' },
          { value: 'pseudo', title: 'Pseudo' },
        ],
        dynamicTitle: true,
      },
    },
  },
  initialGlobals: {
    theme: 'system',
    locale: 'ko-KR',
  },
  parameters: {
    options: {
      storySort: {
        method: 'alphabetical',
        order: [
          'Design System',
          ['Foundations'],
          'Core',
          [
            'Design System',
            ['Foundations', 'Design Language', 'Component Catalog', 'System Controls'],
            '*',
          ],
          'Features',
          ['Builtin Effects', ['All Effects · 실제 노드 전체', '*'], 'Routing', '*'],
          'Screens',
          'App',
          ['Workspace', ['Workspace', 'All Effects', '*']],
        ],
      },
    },
    docs: {
      story: { inline: false, height: '480px' },
      toc: {
        headingSelector: 'h2, h3',
      },
    },
    a11y: {
      test: 'error',
    },
    controls: {
      expanded: true,
    },
    viewport: {
      options: {
        mobile: { name: 'Mobile narrow', styles: { width: '390px', height: '844px' } },
        tablet: { name: 'Tablet', styles: { width: '768px', height: '1024px' } },
        desktop: { name: 'Desktop', styles: { width: '1280px', height: '800px' } },
      },
    },
  },
};

export default preview;
