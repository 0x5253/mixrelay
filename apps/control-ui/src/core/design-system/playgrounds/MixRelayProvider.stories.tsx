import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { MixRelayProvider } from '../../theme/MixRelayProvider';
import { Text } from '../../ui/Text';
import { ThemePicker } from '../../ui/ThemePicker';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/MixRelayProvider',
  component: MixRelayProvider,
  argTypes: {
    theme: { control: 'select', options: ['system', 'light', 'dark'] },
    onThemeChange: { control: false },
    children: { control: false },
  },
  args: {
    theme: 'system',
    onThemeChange: fn(),
    children: (
      <>
        <ThemePicker />
        <Text>같은 테마와 글꼴을 소비하는 Core</Text>
      </>
    ),
  },
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
          '이 provider는 문서 Canvas의 iframe에서만 테마를 소유합니다. 조합 시 앱은 최상위 provider 하나를 사용합니다.',
      },
    },
  },
  render: function Interactive(args) {
    const { local, set, commit } = usePlaygroundState(args);
    return (
      <fieldset
        aria-label="Playground 예제"
        onBlur={commit}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        <MixRelayProvider
          {...local}
          onThemeChange={(theme) => {
            local.onThemeChange?.(theme);
            set({ theme });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof MixRelayProvider>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Light: Story = { args: { theme: 'light' } };
export const Dark: Story = { globals: { theme: 'dark' }, args: { theme: 'dark' } };
