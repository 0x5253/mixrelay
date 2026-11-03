import type { Meta, StoryObj } from '@storybook/react-vite';
import { Inline } from '../../ui/Inline';

import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/Inline',
  component: Inline,
  argTypes: {
    align: { control: 'select', options: ['start', 'center', 'baseline'] },
    justify: { control: 'select', options: ['start', 'center', 'end', 'between'] },
    gap: { control: 'select', options: ['tight', 'regular', 'section', 'wide'] },
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
    align: 'center',
    justify: 'start',
    wrap: true,
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
} satisfies Meta<typeof Inline>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Wrap: Story = {
  args: {
    children: (
      <>
        <Text>긴 항목 · 첫 번째 표본</Text>
        <Text>긴 항목 · 두 번째 표본</Text>
        <Text>긴 항목 · 세 번째 표본</Text>
      </>
    ),
  },
};
export const Between: Story = { args: { justify: 'between' } };
export const Centered: Story = { args: { justify: 'center' } };
export const Baseline: Story = { args: { align: 'baseline' } };
