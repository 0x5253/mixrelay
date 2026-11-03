import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';
import { ActionButton } from './ActionButton';

const meta = {
  title: 'Core/Controls/ActionButton',
  component: ActionButton,
  args: {
    children: '그래프 생성',
    tone: 'neutral',
    density: 'regular',
    emphasis: 'quiet',
    stretch: false,
    align: 'center',
    loading: false,
    disabled: false,
    onClick: fn(),
  },
  argTypes: {
    children: {
      control: 'text',
      description: '동작을 설명하는 이름. 이 예시는 텍스트를 입력합니다.',
    },
    tone: { control: 'select', options: ['neutral', 'critical'] },
    density: { control: 'select', options: ['regular', 'compact'] },
    emphasis: { control: 'select', options: ['quiet', 'outlined', 'strong'] },
    stretch: { control: 'boolean' },
    align: { control: 'select', options: ['center', 'start'] },
    loading: { control: 'boolean', description: '처리 중: busy를 읽고 추가 실행을 막습니다.' },
    disabled: { control: 'boolean', description: '현재 사용할 수 없는 명령.' },
    onClick: { control: false, description: '실제 명령 소유자가 전달하는 클릭 이벤트.' },
  },
  parameters: {
    controls: {
      include: [
        'children',
        'tone',
        'density',
        'emphasis',
        'stretch',
        'align',
        'loading',
        'disabled',
      ],
    },
    docs: { source: { type: 'dynamic' } },
  },
} satisfies Meta<typeof ActionButton>;

export default meta;
type Story = StoryObj<typeof meta>;

export const Playground: Story = {};
export const Pressed: Story = {
  args: { children: 'MIDI', density: 'compact', 'aria-pressed': true },
};
export const Submit: Story = { args: { emphasis: 'strong' } };
export const Loading: Story = { args: { emphasis: 'strong', loading: true } };
export const Disabled: Story = { args: { disabled: true } };
export const Destructive: Story = { args: { children: '그래프 삭제', tone: 'critical' } };
export const NarrowLongLabel: Story = {
  args: { children: 'Open the complete MixRelay installer · universal — expanded translation' },
  render: (args) => (
    <div style={{ width: 280, maxWidth: '100%' }}>
      <ActionButton {...args} />
    </div>
  ),
};
export const Keyboard: Story = {
  play: async ({ canvasElement, args }) => {
    const button = within(canvasElement).getByRole('button', { name: String(args.children) });
    await userEvent.tab();
    await expect(button).toHaveFocus();
    await userEvent.keyboard('{Enter}');
    await userEvent.keyboard(' ');
    await expect(args.onClick).toHaveBeenCalledTimes(2);
  },
};
