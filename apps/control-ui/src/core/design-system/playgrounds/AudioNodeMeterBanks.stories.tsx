import type { Meta, StoryObj } from '@storybook/react-vite';
import { AudioMeterProvider } from '../../audio-meter/AudioMeterProvider';
import { AudioNodeMeterBanks } from '../../ui/AudioNodeMeterBanks';
import { labelledMeterBanks, meterBanks, nodeMeterStore } from './fixtures';

const meta = {
  title: 'Core/Playgrounds/AudioNodeMeterBanks',
  component: AudioNodeMeterBanks,
  args: { banks: meterBanks, offline: false, title: 'Studio monitor' },
  decorators: [
    (Story) => (
      <AudioMeterProvider store={nodeMeterStore}>
        <div style={{ width: 208 }}>
          <Story />
        </div>
      </AudioMeterProvider>
    ),
  ],
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
          '노드 출력의 실제 레벨을 채널별 읽기 전용 미터로 표시합니다. offline이면 값을 비우고 같은 배치를 유지합니다. 8채널을 넘으면 세로 미터 격자로 바뀝니다.',
      },
    },
  },
} satisfies Meta<typeof AudioNodeMeterBanks>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Playground: Story = {};
export const Labelled: Story = { args: { banks: labelledMeterBanks } };
export const Multichannel: Story = {
  args: { banks: [{ channels: 16, nodeId: 'sample-meter', portId: 'surround' }] },
};
export const Offline: Story = { args: { offline: true } };
