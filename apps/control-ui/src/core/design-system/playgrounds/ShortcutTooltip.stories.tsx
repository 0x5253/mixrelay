import type { Meta, StoryObj } from '@storybook/react-vite';
import { ShortcutTooltip } from '../../ui/ShortcutTooltip';

const meta = {
  title: 'Core/Playgrounds/ShortcutTooltip',
  component: ShortcutTooltip,
  argTypes: {
    children: { control: false },
  },
  args: { shortcut: 'Mod+S', children: <button type="button">샘플 저장</button> },
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
          'Controls는 실제 공개 props를 전달합니다. 샘플 callback은 Actions에 기록하며 제품 요청을 실행하지 않습니다.',
      },
    },
  },
} satisfies Meta<typeof ShortcutTooltip>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Alternate: Story = { args: { shortcut: 'Mod+Shift+S' } };
