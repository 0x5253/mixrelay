import type { Meta, StoryObj } from '@storybook/react-vite';
import { ThemePicker } from '../../ui/ThemePicker';

const meta = {
  title: 'Core/Playgrounds/ThemePicker',
  component: ThemePicker,
  argTypes: {},
  args: {},
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
          '공개 props가 없습니다. 전역 toolbar·ThemeContext를 소비하며 메뉴 선택은 Storybook의 테마를 변경합니다.',
      },
    },
  },
} satisfies Meta<typeof ThemePicker>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Light: Story = { globals: { theme: 'light' }, args: {} };
export const Dark: Story = { globals: { theme: 'dark' }, args: {} };
