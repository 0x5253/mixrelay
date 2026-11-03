import type { Meta, StoryObj } from '@storybook/react-vite';
import {
  type BuiltinEffectFixtureKind,
  builtinEffectsFixture,
} from '../../../.storybook/fixtures/builtinEffects';
import { RoutingWorkspace, type RoutingWorkspaceProps } from './RoutingWorkspace';

const meta = {
  title: 'Features/Builtin Effects',
  component: RoutingWorkspace,
  args: { graphId: 'graph-1' },
  loaders: [({ parameters }) => ({ controlPort: builtinEffectsFixture(parameters.effectFixture) })],
  render: (args, { loaded }) => <RoutingWorkspace {...args} controlPort={loaded.controlPort} />,
  decorators: [
    (Story) => (
      <div style={{ height: '100dvh', minHeight: 320 }}>
        <Story />
      </div>
    ),
  ],
  parameters: {
    layout: 'fullscreen',
    docs: {
      description: {
        component:
          '제품과 같은 RoutingWorkspace → AudioGraphCanvas → AudioNodeCard를 사용합니다. 문서·배치·최초 계측만 fixture이며, 파라미터·프리셋·바이패스·옵션은 제품 명령 경로로 조작합니다. 고정 계측은 실제 오디오가 아니며 설정 변경 후에는 이전 revision 계측을 표시하지 않습니다.',
      },
    },
  },
} satisfies Meta<typeof RoutingWorkspace>;
export default meta;
// The required control port is supplied by the loader, not serializable story args.
type Story = StoryObj<Partial<RoutingWorkspaceProps>>;

const effect = (kind: BuiltinEffectFixtureKind, options = {}): Story => ({
  parameters: { effectFixture: { kinds: [kind], ...options } },
});

export const AllEffects: Story = { name: 'All Effects · 실제 노드 전체' };
export const AllEffectsDark: Story = { globals: { theme: 'dark' } };
export const AllEffectsNarrow: Story = { globals: { viewport: { value: 'mobile' } } };
export const AllEffectsLongTranslation: Story = { globals: { locale: 'pseudo' } };
export const Dynamics: Story = {
  parameters: { effectFixture: { kinds: ['compressor', 'limiter'] } },
};
export const DynamicsWithoutTelemetry: Story = {
  parameters: { effectFixture: { kinds: ['compressor', 'limiter'], telemetry: false } },
};
export const Compressor: Story = effect('compressor');
export const Limiter: Story = effect('limiter');
export const Equalizer: Story = effect('equalizer');
export const EqualizerBypassed: Story = effect('equalizer', { bypass: true });
export const HighPass: Story = effect('high_pass');
export const LowPass: Story = effect('low_pass');
export const Gate: Story = effect('gate');
export const Expander: Story = effect('expander');
export const ChannelUtility: Story = effect('channel_utility');
export const MonoPan: Story = effect('pan');
export const StereoBalance: Story = effect('balance');
export const Ducking: Story = effect('ducking');
export const Reverb: Story = effect('reverb');
export const ReverbHall: Story = effect('reverb', { preset: 'hall' });
export const ReverbTunnel: Story = effect('reverb', { preset: 'tunnel' });
export const ReverbResized: Story = effect('reverb', { height: 240 });
export const Delay: Story = effect('delay');
export const DelayPingPong: Story = effect('delay', { preset: 'pingPong' });
export const Chorus: Story = effect('chorus');
