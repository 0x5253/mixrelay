import type { Meta, StoryObj } from '@storybook/react-vite';
import { useTranslation } from 'react-i18next';
import { I18nProvider } from '../../i18n';
import { Text } from '../../ui/Text';

function CurrentLanguage() {
  const { t, i18n } = useTranslation();
  return (
    <Text>
      {i18n.resolvedLanguage} · {t('routing.nodes.options')} · {t('studio.newGraph')}
    </Text>
  );
}

const meta = {
  title: 'Core/Playgrounds/I18nProvider',
  component: I18nProvider,
  argTypes: { children: { control: false } },
  args: { children: <CurrentLanguage /> },
  parameters: {
    controls: {
      exclude: [
        'className',
        'classNames',
        'style',
        'styles',
        'vars',
        'unstyled',
        'color',
        'c',
        'size',
        'radius',
        'variant',
        'component',
        'renderRoot',
        'dangerouslySetInnerHTML',
      ],
    },
    docs: {
      source: { type: 'dynamic' },
      description: {
        component:
          '앱의 i18n 인스턴스를 공급하고 html lang을 현재 언어로 맞춥니다. 언어는 Storybook toolbar의 locale로 바꿉니다.',
      },
    },
  },
} satisfies Meta<typeof I18nProvider>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const English: Story = { globals: { locale: 'en' } };
export const Pseudo: Story = { globals: { locale: 'pseudo' } };
