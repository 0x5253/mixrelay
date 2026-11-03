import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { IconActionButton } from '../../ui/IconActionButton';

const meta = {
  title: 'Core/Playgrounds/IconActionButton',
  component: IconActionButton,
  argTypes: {
    tone: { control: 'select', options: ['neutral', 'critical', 'warning'] },
    density: { control: 'select', options: ['regular', 'compact'] },
    children: { control: false },
    onClick: { control: false },
  },
  args: {
    label: '노드 추가',
    children: (
      <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden="true">
        <path d="M8 2v12M2 8h12" stroke="currentColor" />
      </svg>
    ),
    tone: 'neutral',
    density: 'compact',
    indicator: false,
    disabled: false,
    onClick: fn(),
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
} satisfies Meta<typeof IconActionButton>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Disabled: Story = { args: { disabled: true } };
export const Warning: Story = { args: { tone: 'warning', indicator: true } };
export const Critical: Story = { args: { tone: 'critical', density: 'regular' } };
