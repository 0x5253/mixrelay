import type { Meta, StoryObj } from '@storybook/react-vite';
import { ProcessorTimingLabel } from '../../ui/ProcessorTimingLabel';

const meta = {
  title: 'Core/Playgrounds/ProcessorTimingLabel',
  component: ProcessorTimingLabel,
  argTypes: {},
  args: { nanoseconds: 125000 },
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
      description: { component: '값이 없거나 유한한 양수가 아니면 아무것도 렌더하지 않습니다.' },
    },
  },
} satisfies Meta<typeof ProcessorTimingLabel>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Milliseconds: Story = { args: { nanoseconds: 2300000 } };
export const Absent: Story = { args: { nanoseconds: undefined } };
export const NonPositive: Story = { args: { nanoseconds: 0 } };
