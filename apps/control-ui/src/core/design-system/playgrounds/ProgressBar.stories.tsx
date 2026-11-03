import type { Meta, StoryObj } from '@storybook/react-vite';
import { ProgressBar } from '../../ui/ProgressBar';

const meta = {
  title: 'Core/Playgrounds/ProgressBar',
  component: ProgressBar,
  args: { label: 'MixRelay를 시작하는 중', value: 1, maxValue: 3, valueLabel: '단계 2/3' },
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
          '지금 하는 일(label)과 진행 정도(valueLabel, 막대)를 함께 표시합니다. React Aria ProgressBar가 progressbar 역할과 값을 제공합니다. reduced motion에서는 막대 전환이 즉시 끝납니다.',
      },
    },
  },
} satisfies Meta<typeof ProgressBar>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Started: Story = {
  args: { label: 'MixRelay 확인 중', value: 0, valueLabel: '단계 1/3' },
};
export const LongLabel: Story = {
  args: {
    label: 'MixRelay 오디오 엔진과 드라이버의 준비 상태를 확인하는 중입니다',
    value: 2,
    valueLabel: '단계 3/3',
  },
};
