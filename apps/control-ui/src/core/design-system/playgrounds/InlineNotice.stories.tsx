import type { Meta, StoryObj } from '@storybook/react-vite';
import { ActionButton } from '../../ui/ActionButton';
import { InlineNotice } from '../../ui/InlineNotice';

const meta = {
  title: 'Core/Playgrounds/InlineNotice',
  component: InlineNotice,
  argTypes: {
    announcement: { control: 'select', options: ['status', 'alert'] },
    tone: { control: 'select', options: ['neutral', 'accent', 'positive', 'warning', 'critical'] },
  },
  args: {
    children: '마지막 확인값을 유지합니다.',
    title: '설정 안내',
    tone: 'neutral',
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
} satisfies Meta<typeof InlineNotice>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Critical: Story = { args: { tone: 'critical' } };
export const WarningAlert: Story = { args: { tone: 'warning', announcement: 'alert' } };
export const WithAction: Story = {
  args: { tone: 'critical', actions: <ActionButton>다시 시도</ActionButton> },
};
