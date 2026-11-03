import type { Meta, StoryObj } from '@storybook/react-vite';
import { fn } from 'storybook/test';
import { AudioGraphCanvas } from '../../audio-graph/AudioGraphCanvas';
import { sampleGraph } from './fixtures';
import { usePlaygroundState } from './usePlaygroundState';

const meta = {
  title: 'Core/Playgrounds/AudioGraphCanvas',
  component: AudioGraphCanvas,
  argTypes: {
    onModelChange: { control: false },
    onNodeRenameRequest: { control: false },
  },
  args: {
    ariaLabel: '샘플 라우팅 그래프',
    model: sampleGraph,
    onModelChange: fn(),
    onNodeRenameRequest: fn(),
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
          '동일 React Flow 편집기를 사용합니다. 변경은 이 표본의 모델에만 저장하며 오디오 포트를 사용하지 않습니다.',
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
        <div style={{ height: 520, width: '100%' }}>
          <AudioGraphCanvas
            {...local}
            onModelChange={(model, details) => {
              local.onModelChange(model, details);
              set({ model });
            }}
          />
        </div>
      </fieldset>
    );
  },
} satisfies Meta<typeof AudioGraphCanvas>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Empty: Story = { args: { model: { ...sampleGraph, nodes: [], edges: [] } } };
