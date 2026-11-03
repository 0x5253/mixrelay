import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { TransportButton } from '../../ui/TransportButton';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/TransportButton',
  component: TransportButton,
  argTypes: {
    icon: { control: 'select', options: ['record', 'pause', 'stop', 'resume'] },
    tone: { control: 'select', options: ['neutral', 'accent', 'positive', 'warning', 'critical'] },
    onClick: { control: false },
  },
  args: {
    icon: 'record',
    label: '샘플 녹음 시작',
    active: false,
    disabled: false,
    busy: false,
    compact: false,
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
        component: '조작은 샘플 active만 변경합니다. 녹음을 시작하거나 파일을 쓰지 않습니다.',
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
        <TransportButton
          {...local}
          onClick={() => {
            local.onClick();
            set({ active: !local.active });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof TransportButton>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Active: Story = { args: { active: true, pressed: true } };
export const Busy: Story = { args: { busy: true, disabled: true } };
export const Pause: Story = { args: { icon: 'pause', active: true } };
