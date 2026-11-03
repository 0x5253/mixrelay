import type { Meta, StoryObj } from '@storybook/react-vite';
import { ActionButton } from '../../ui/ActionButton';
import { NodeTransportRow } from '../../ui/NodeTransportRow';

const meta = {
  title: 'Core/Playgrounds/NodeTransportRow',
  component: NodeTransportRow,
  argTypes: {
    children: { control: false },
  },
  args: {
    label: '녹음 시간',
    value: '00:12',
    state: '녹음 중',
    description: '샘플 시간입니다.',
    children: <ActionButton>일시정지</ActionButton>,
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
} satisfies Meta<typeof NodeTransportRow>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Paused: Story = { args: { state: '일시정지' } };
export const LongState: Story = {
  args: { state: '저장 위치를 확인한 뒤 다시 시작할 수 있습니다.' },
};
