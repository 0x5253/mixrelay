import type { Meta, StoryObj } from '@storybook/react-vite';
import { Text } from '../../ui/Text';

const meta = {
  title: 'Core/Playgrounds/Text',
  component: Text,
  argTypes: {
    kind: { control: 'select', options: ['heading', 'body', 'supporting', 'label', 'caption'] },
    as: { control: 'select', options: ['h1', 'h2', 'p', 'span', 'small'] },
    tone: {
      control: 'select',
      options: ['neutral', 'accent', 'positive', 'warning', 'critical', 'muted'],
    },
  },
  args: { children: '설정과 현재 값을 구별해 표시합니다.', kind: 'body', tone: 'neutral', as: 'p' },
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
} satisfies Meta<typeof Text>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Supporting: Story = { args: { kind: 'supporting', tone: 'muted' } };
export const Critical: Story = { args: { tone: 'critical' } };
export const Caption: Story = { args: { kind: 'caption', as: 'small' } };
export const Heading: Story = {
  args: { kind: 'heading', as: 'h1', children: 'MixRelay에 연결할 수 없습니다' },
};
