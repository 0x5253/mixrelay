import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { RetryAlert } from '../../ui/RetryAlert';

const meta = {
  title: 'Core/Playgrounds/RetryAlert',
  component: RetryAlert,
  argTypes: {
    onRetry: { control: false },
  },
  args: {
    message: '설정을 적용하지 못했습니다. 마지막 값을 유지합니다.',
    retryLabel: '다시 시도',
    busy: false,
    onRetry: fn(),
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
} satisfies Meta<typeof RetryAlert>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Busy: Story = { args: { busy: true } };
