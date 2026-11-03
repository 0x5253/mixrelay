import type { Meta, StoryObj } from '@storybook/react-vite';
import { StatusIndicator } from '../../ui/StatusIndicator';

const meta = {
  title: 'Core/Playgrounds/StatusIndicator',
  component: StatusIndicator,
  args: { label: '확인 완료', tone: 'positive', position: 'top' },
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
          '표식의 크기와 상태색은 Core `--status-indicator-*`와 상태 역할이 소유합니다. 사용처는 label·tone·툴팁 위치만 전달합니다.',
      },
    },
  },
} satisfies Meta<typeof StatusIndicator>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Critical: Story = { args: { label: '조회 실패', tone: 'critical' } };
export const Warning: Story = { args: { label: '확인 필요', tone: 'warning' } };
export const Neutral: Story = { args: { label: '상태 미확인', tone: 'neutral' } };
