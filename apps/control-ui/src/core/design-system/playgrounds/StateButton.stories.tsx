import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { StateButton } from '../../ui/StateButton';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/StateButton',
  component: StateButton,
  argTypes: {
    tone: { control: 'select', options: ['neutral', 'accent', 'positive', 'warning', 'critical'] },
    value: { control: 'select', options: ['idle', 'pending', 'failed'] },
    onClick: { control: false },
  },
  args: {
    states: [
      { value: 'idle', label: '실행', icon: null },
      { value: 'pending', label: '확인 중', icon: null },
      { value: 'failed', label: '실패', icon: null },
    ],
    value: 'idle',
    tone: 'neutral',
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
  render: function Interactive(args) {
    const { local, set, commit } = usePlaygroundState(args);
    return (
      <fieldset
        aria-label="Playground 예제"
        onBlur={commit}
        style={{ border: 0, padding: 0, margin: 0, minWidth: 0 }}
      >
        <StateButton
          {...local}
          onClick={(event) => {
            local.onClick?.(event);
            set({ value: local.value === 'idle' ? 'pending' : 'idle' });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof StateButton>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Pending: Story = { args: { value: 'pending', disabled: true, 'aria-busy': true } };
export const Failed: Story = { args: { value: 'failed', tone: 'critical' } };
