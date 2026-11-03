import type { Meta, StoryObj } from '@storybook/react-vite';
import { PortSelectionFeedback } from '../../audio-graph/PortSelectionFeedback';
import { sampleGraph } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/PortSelectionFeedback',
  component: PortSelectionFeedback,
  argTypes: {},
  args: {
    model: sampleGraph,
    ports: [{ nodeId: 'source', portId: 'out-1', side: 'output' }],
    position: { x: 24, y: 24 },
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
  render: (args) => (
    <div style={{ position: 'relative', height: 120 }}>
      <PortSelectionFeedback {...args} />
    </div>
  ),
} satisfies Meta<typeof PortSelectionFeedback>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const NoSelection: Story = { args: { ports: [] } };
export const Inactive: Story = { args: { position: null } };
