import type { Meta, StoryObj } from '@storybook/react-vite';
import { Stack } from '../../ui/Stack';

import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/Stack',
  component: Stack,
  argTypes: {
    gap: { control: 'select', options: ['tight', 'regular', 'section', 'wide'] },
    align: { control: 'inline-radio', options: ['stretch', 'center'] },
    children: { control: false },
  },
  args: {
    children: (
      <>
        <Text>첫 번째 항목</Text>
        <Text>두 번째 항목</Text>
      </>
    ),
    gap: 'regular',
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
          'Controls는 실제 공개 props를 전달합니다. 샘플 callback은 Actions에 기록하며 제품 요청을 실행하지 않습니다.',
      },
    },
  },
} satisfies Meta<typeof Stack>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Tight: Story = { args: { gap: 'tight' } };
export const Wide: Story = { args: { gap: 'wide' } };
export const Centered: Story = { args: { align: 'center' } };
