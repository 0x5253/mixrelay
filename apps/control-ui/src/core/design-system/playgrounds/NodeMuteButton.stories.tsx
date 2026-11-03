import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { NodeMuteButton } from '../../ui/NodeSignalControls';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/NodeMuteButton',
  component: NodeMuteButton,
  argTypes: {
    onChange: { control: false },
  },
  args: { muted: false, disabled: false, label: '노드 음소거 변경', onChange: fn() },
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
        <NodeMuteButton
          {...local}
          onChange={() => {
            local.onChange();
            set({ muted: !local.muted });
          }}
        />
      </fieldset>
    );
  },
} satisfies Meta<typeof NodeMuteButton>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Active: Story = { args: { muted: true } };
export const Disabled: Story = { args: { muted: true, disabled: true } };
