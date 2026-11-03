import type { Meta, StoryObj } from '@storybook/react-vite';
import { Meter } from '../../ui/Meter';

const meta = {
  title: 'Core/Playgrounds/Meter',
  component: Meter,
  argTypes: { orientation: { control: 'select', options: ['horizontal', 'vertical'] } },
  args: { label: '출력 레벨', level: 48, orientation: 'vertical' },
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
        component: 'level은 이미 정규화된 0–100 표시값입니다. 오디오 진폭이나 dB 입력이 아닙니다.',
      },
    },
  },
  render: (args) => (
    <div style={{ height: 120, display: 'flex' }}>
      <Meter {...args} />
    </div>
  ),
} satisfies Meta<typeof Meter>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Zero: Story = { args: { level: 0 } };
export const Clamped: Story = { args: { level: 120 } };
export const Horizontal: Story = { args: { orientation: 'horizontal' } };
