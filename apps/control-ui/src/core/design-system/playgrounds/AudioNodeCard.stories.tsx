import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, within } from 'storybook/test';
import appIcon from '../../../../../desktop/src-tauri/icons/32x32.png';
import { AudioNodeCard } from '../../ui/AudioNodeCard';

const meta = {
  title: 'Core/Playgrounds/AudioNodeCard',
  component: AudioNodeCard,
  argTypes: {
    onSelect: { control: false },
  },
  args: {
    category: 'Application',
    icon: 'APP',
    iconUrl: appIcon,
    title: 'MixRelay',
    inputCount: 0,
    outputCount: 2,
    channelCount: 2,
    poweredOff: false,
    bypassed: false,
    offline: false,
    selected: false,
    onSelect: fn(),
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
    <div style={{ width: 'var(--node-default-width)', maxWidth: '100%' }}>
      <AudioNodeCard {...args} />
    </div>
  ),
} satisfies Meta<typeof AudioNodeCard>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Off: Story = { args: { poweredOff: true } };
export const Bypassed: Story = { args: { bypassed: true } };
export const Offline: Story = { args: { offline: true } };
export const Selected: Story = { args: { selected: true } };
export const Fallback: Story = { args: { iconUrl: undefined } };

export const CompactFailure: Story = {
  args: {
    title: 'Voice effect',
    category: 'Processor',
    icon: 'FX',
    iconUrl: undefined,
    inputCount: 2,
    outputCount: 2,
    faceMode: 'compact',
    status: 'Plug-in processing stopped',
    statusTone: 'critical',
    detail: 'Plug-in processing stopped',
    onSelect: undefined,
  },
  play: async ({ canvasElement }) => {
    await expect(within(canvasElement).getByText('Plug-in processing stopped')).toBeVisible();
  },
};
export const CompactFailureNarrow: Story = {
  ...CompactFailure,
  args: {
    ...CompactFailure.args,
    status: 'Plug-in processing stopped · Verarbeitung des externen Plug-ins wurde unterbrochen',
    detail: 'Plug-in processing stopped · Verarbeitung des externen Plug-ins wurde unterbrochen',
  },
  play: undefined,
  decorators: [
    (Story) => (
      <div style={{ width: 180 }}>
        <Story />
      </div>
    ),
  ],
};

/** A plug-in whose channel layout is chosen on the node, not in the Add node menu. */
export const ChannelLayoutChoice: Story = {
  args: {
    category: 'Processor · AU',
    icon: 'AU',
    iconUrl: undefined,
    title: 'AUBandpass',
    inputCount: 2,
    outputCount: 2,
    channelLayoutControl: {
      label: '채널 구성',
      value: 'stereo',
      options: [
        { value: 'mono', label: '모노' },
        { value: 'stereo', label: '스테레오' },
        { value: 'six', label: '6채널' },
      ],
    },
    onChannelLayoutChange: fn(),
  },
};
