import type { Meta, StoryObj } from '@storybook/react-vite';
import { ContentSection } from '../../ui/ContentSection';

import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/ContentSection',
  component: ContentSection,
  argTypes: {
    level: { control: 'select', options: [2, 3] },
    divider: { control: 'select', options: ['none', 'before', 'after'] },
    children: { control: false },
  },
  args: {
    title: '연결 상태',
    level: 2,
    divider: 'none',
    children: <Text>실행부에서 확인한 상태입니다.</Text>,
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
} satisfies Meta<typeof ContentSection>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Before: Story = { args: { divider: 'before' } };
export const After: Story = { args: { divider: 'after', level: 3 } };
