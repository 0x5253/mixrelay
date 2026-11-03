import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { RecoveryActions } from '../../ui/RecoveryActions';

const meta = {
  title: 'Core/Playgrounds/RecoveryActions',
  component: RecoveryActions,
  argTypes: {
    onRetry: { control: false },
    onReload: { control: false },
  },
  args: {
    retryLabel: '편집본 적용',
    reloadLabel: '편집 취소 후 불러오기',
    onRetry: fn(),
    onReload: fn(),
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
          '자동 해결할 수 없는 지속 충돌용입니다. 일반 조회·통신 실패는 RetryAlert 하나를 사용합니다.',
      },
    },
  },
} satisfies Meta<typeof RecoveryActions>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const LongLabels: Story = {
  args: {
    retryLabel: '현재 편집본을 최신 문서에 병합해 적용',
    reloadLabel: '보존한 편집을 취소하고 최신 문서 불러오기',
  },
};
