import type { StorybookConfig } from '@storybook/react-vite';
import remarkGfm from 'remark-gfm';

const STORYBOOK_CHUNK_WARNING_LIMIT_KIB = 1_300;

const config: StorybookConfig = {
  stories: ['../src/**/*.mdx', '../src/**/*.stories.@(ts|tsx)'],
  addons: [
    {
      name: '@storybook/addon-docs',
      options: { mdxPluginOptions: { mdxCompileOptions: { remarkPlugins: [remarkGfm] } } },
    },
    '@storybook/addon-a11y',
  ],
  features: {
    sidebarOnboardingChecklist: false,
    menuOnboardingChecklist: false,
  },
  framework: '@storybook/react-vite',
  viteFinal: (viteConfig) => ({
    ...viteConfig,
    build: {
      ...viteConfig.build,
      chunkSizeWarningLimit: STORYBOOK_CHUNK_WARNING_LIMIT_KIB,
    },
  }),
};

export default config;
