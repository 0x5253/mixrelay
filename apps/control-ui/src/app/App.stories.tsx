import type { Meta, StoryObj } from '@storybook/react-vite';
import { expect, fn, userEvent, within } from 'storybook/test';
import { builtinEffectsFixture } from '../../.storybook/fixtures/builtinEffects';
import { processorReportState } from '../../.storybook/fixtures/processorReport';
import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import quickTimeIcon from '../core/audio-graph/fixtures/quicktime-player.png';
import { ControlPortError } from '../core/domain/control/errors';
import type { RuntimeGraphDocument } from '../core/domain/control/graph';
import type { NativeSampleFormat } from '../core/domain/control/nativeFormat';
import type { MixRelayStatus } from '../core/domain/control/status';
import { i18n } from '../core/i18n';
import { readyDriver } from '../features/driver-status/driverFixtures';
import { midiBinding, midiFixture, midiPortFixture } from '../features/midi/midiFixtures';
import { App } from './App';

const graph: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  nodes: [
    {
      id: 'app',
      label: 'QuickTime Player',
      type: 'application_capture',
      config: {
        selector: {
          match: 'bundle_id',
          bundle_id: 'com.apple.QuickTimePlayer',
          include_descendants: true,
        },
        sourcePlayback: 'preserve',
        stream: { channels: 2, channelLayout: 'stereo' },
        gainDb: 0,
      },
    },
    { id: 'trim', label: 'Monitor trim', type: 'gain', config: { channels: 2, gainDb: -3 } },
    {
      id: 'device-a',
      label: 'Monitor',
      type: 'virtual_endpoint',
      config: {
        ...readyDriver.settings.devices[0]?.spec,
        reference: { render: true, capture: true },
      },
    },
    { id: 'recorder', label: 'Recorder', type: 'recorder', config: { channels: 2 } },
  ],
  edges: [
    {
      id: 'app-trim',
      source: { nodeId: 'app', portId: 'audio' },
      destination: { nodeId: 'trim', portId: 'audio' },
      channelMapping: { mode: 'auto' },
    },
    {
      id: 'trim-monitor',
      source: { nodeId: 'trim', portId: 'audio' },
      destination: { nodeId: 'device-a', portId: 'to_os' },
      channelMapping: { mode: 'auto' },
    },
    {
      id: 'trim-recorder',
      source: { nodeId: 'trim', portId: 'audio' },
      destination: { nodeId: 'recorder', portId: 'audio' },
      channelMapping: { mode: 'auto' },
    },
  ],
};

const workspaceDriver = structuredClone(readyDriver);
const workspaceDevice = workspaceDriver.settings.devices[0];
if (!workspaceDevice) throw new Error('Workspace preview requires a device.');
const workspaceChannels = workspaceDevice.spec.render?.channels;
if (!workspaceChannels) throw new Error('Workspace preview requires a playback stream.');
const workspaceEncodings: NativeSampleFormat[] = ['signed16', 'signed24', 'signed32', 'float32'];
workspaceDevice.nativeFormats.render.supported = workspaceDevice.spec.supportedSampleRates.flatMap(
  (sampleRate) =>
    workspaceEncodings.map((sampleFormat) => ({
      sampleRate,
      channels: workspaceChannels,
      sampleFormat,
    })),
);
workspaceDevice.nativeFormats.capture = structuredClone(workspaceDevice.nativeFormats.render);

const status: MixRelayStatus = {
  protocolVersion: 1,
  driverState: 'ready',
  sampleRate: 48000,
  bufferFrames: 128,
  underruns: 0,
  overruns: 0,
  enginePower: { enabled: true, transitioning: false },
  audioSession: {
    virtualEndpointSettings: workspaceDriver.settings,
    processingState: 'running',
    issue: null,
    format: { sampleRate: 48000, blockFrames: 128 },
    clockAuthority: 'engine_timer',
    observedDeviceSampleRate: null,
    transportLatencyFrames: null,
    capabilities: {
      backend: 'core_audio',
      supportedSampleRates: [48000],
      supportedBlockFrames: [128],
      canReconfigureLive: false,
      hardwareInput: true,
      hardwareOutput: true,
      virtualEndpoint: true,
      applicationCapture: true,
      sampleRateConversion: false,
      driftCorrection: false,
    },
  },
};

async function workspaceFixture({ parameters }: { parameters: Record<string, unknown> }) {
  if (parameters.allEffects) return { controlPort: builtinEffectsFixture() };
  const state = {
    graph: structuredClone(graph),
    processors: parameters.processorScanIssues ? processorReportState().processors : undefined,
    graphStatus: { protocolVersion: 1 as const, activeRevision: 1, submittedRevision: 1 },
    status: structuredClone(status),
    applications: [
      {
        bundleId: 'com.apple.QuickTimePlayer',
        displayName: 'QuickTime Player',
        iconDataUrl: quickTimeIcon,
        processId: 42,
        selector: {
          match: 'bundle_id' as const,
          bundle_id: 'com.apple.QuickTimePlayer',
          include_descendants: true,
        },
      },
    ],
    meters: {
      protocolVersion: 1 as const,
      revision: 1,
      nodes: [
        { nodeId: 'app', portId: 'audio', peaks: [0.4, 0.28] },
        { nodeId: 'trim', portId: 'audio', peaks: [0.28, 0.2] },
        { nodeId: 'device-a', portId: 'to_os', peaks: [0.28, 0.2] },
      ],
    },
  };
  if (parameters.emptyGraph) {
    state.graph.nodes = [];
    state.graph.edges = [];
    state.meters.nodes = [];
  }
  if (parameters.playbackPaused || parameters.pausedAfterFailures) {
    state.status.enginePower = {
      enabled: false,
      transitioning: false,
      ...(parameters.pausedAfterFailures ? { pauseReason: 'repeatedFailures' as const } : {}),
    };
    state.meters.nodes = [];
  }
  const port = new StorybookMockControlPort(state);
  const controlPort = Object.assign(port, {
    setEnginePower: async (enabled: boolean) => {
      state.status.enginePower = {
        enabled,
        transitioning: false,
        unsaved: Boolean(parameters.pauseNotSaved),
      };
      return {
        protocolVersion: 1 as const,
        revision: state.graph.revision,
        state: 'active' as const,
      };
    },
  });
  await port.setGraphLayout('graph-1', {
    expectedLayoutRevision: 0,
    layout: {
      positions: {
        app: { x: 20, y: 30 },
        trim: { x: 320, y: 30 },
        'device-a::host-source': { x: 20, y: 280 },
        'device-a::host-sink': { x: 620, y: 30 },
        recorder: { x: 320, y: 280 },
      },
      viewport: { x: 80, y: 80, zoom: 1 },
    },
  });
  if (parameters.longGraphList) await populateGraphList(port);
  configureCatalogState(controlPort, parameters);
  return { controlPort };
}

function configureCatalogState(
  port: StorybookMockControlPort,
  parameters: Record<string, unknown>,
) {
  if (parameters.catalogUnavailable)
    port.getGraphCatalog = async () => {
      throw new ControlPortError(503, {
        code: 'control_request_failed',
        parameters: { resource: 'graph_catalog' },
      });
    };
  if (parameters.catalogPending) port.getGraphCatalog = () => new Promise(() => undefined);
  if (parameters.layoutUnavailable) {
    const read = port.getGraphLayout.bind(port);
    let failed = false;
    port.getGraphLayout = async (graphId) => {
      if (failed) return read(graphId);
      failed = true;
      throw new Error('Layout storage is unavailable in this preview.');
    };
  }
}

async function populateGraphList(port: StorybookMockControlPort) {
  for (let index = 1; index <= 32; index += 1) {
    const catalog = await port.getGraphCatalog();
    await port.mutateGraphCatalog({
      action: 'create',
      name: `Session ${index}`,
      expectedRevision: catalog.document.revision,
      expectedCatalogRevision: catalog.catalogRevision,
    });
  }
  const catalog = await port.getGraphCatalog();
  await port.mutateGraphCatalog({
    action: 'select',
    id: 'graph-1',
    expectedRevision: catalog.document.revision,
    expectedCatalogRevision: catalog.catalogRevision,
  });
}

const meta = {
  title: 'App/Workspace',
  component: App,
  args: {
    runtimePort: { connect: async () => 'ready', status: async () => 'ready' },
    driverPort: {
      getSnapshot: async () => workspaceDriver,
      mutate: async () => {
        throw new Error('Native driver changes are unavailable in this preview.');
      },
    },
    updatesPort: null,
    diagnosticsPort: { saveReport: fn(async () => undefined) },
  },
  loaders: [workspaceFixture],
  render: (args, { loaded }) => <App {...args} controlPort={loaded.controlPort} />,
  parameters: {
    layout: 'fullscreen',
    docs: {
      description: {
        component:
          'The production App composition with deterministic preview ports. No native audio or files are changed.',
      },
    },
  },
} satisfies Meta<typeof App>;
export default meta;
type Story = StoryObj<typeof meta>;
export const Workspace: Story = {};
export const StartupRecovery: Story = {
  args: { runtimePort: { connect: async () => 'unavailable', status: async () => 'ready' } },
};
export const AllEffects: Story = {
  parameters: {
    allEffects: true,
    docs: {
      description: {
        story:
          '실제 App 안에서 내장 효과 전체를 검토합니다. 그래프·최초 계측만 fixture이고 효과 조작은 제품의 동일한 Feature를 통과합니다. 실제 오디오 처리 검증은 별도입니다.',
      },
    },
  },
};
export const Light: Story = { globals: { theme: 'light' } };
export const Dark: Story = { globals: { theme: 'dark' } };
export const Empty: Story = {
  parameters: {
    emptyGraph: true,
    docs: {
      description: {
        story: '실제 App의 빈 그래프. 장치 삭제나 오디오 실행은 fixture에서 수행하지 않습니다.',
      },
    },
  },
};
export const AudioPaused: Story = {
  parameters: {
    playbackPaused: true,
    docs: {
      description: {
        story:
          '실행부의 확인된 enginePower.enabled=false를 실제 App이 표시합니다. 노드 Off·Mute·개별 녹음 Pause와 별개입니다. 실제 오디오 중지·재개 검증은 아닙니다.',
      },
    },
  },
};
export const AudioPausedAfterFailures: Story = {
  parameters: {
    pausedAfterFailures: true,
    docs: {
      description: {
        story:
          '엔진이 짧은 시간에 여러 번 비정상 종료되어 스스로 일시정지한 상태입니다. 같은 그래프로 계속 실패하지 않도록 재개 전까지 오디오를 멈추고, 경고(status)로 이유를 알립니다.',
      },
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await expect(await canvas.findByText(i18n.t('audioSession.pausedAfterFailures'))).toBeVisible();
    await expect(
      canvas.getByRole('button', { name: i18n.t('audioSession.resume') }),
    ).toHaveAttribute('aria-pressed', 'true');
  },
};
export const AudioPauseNotSaved: Story = {
  parameters: {
    pauseNotSaved: true,
    docs: {
      description: {
        story:
          '일시정지는 적용됐지만 서비스가 다음 시작을 위해 저장하지 못한 경우입니다. 변경은 유지되므로 경고(status)로 알리고, 실패(alert)로 표시하지 않습니다.',
      },
    },
  },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole('button', { name: i18n.t('audioSession.pause') }),
    );
    // The control waits for the service to confirm the change before reporting it.
    const notice = await canvas.findByText(
      i18n.t('audioSession.pauseNotSaved'),
      {},
      { timeout: 10000 },
    );
    await expect(notice).toBeVisible();
    await expect(
      canvas.getByRole('button', { name: i18n.t('audioSession.resume') }),
    ).toHaveAttribute('aria-pressed', 'true');
  },
};
export const LongGraphList: Story = { parameters: { longGraphList: true } };
export const Connecting: Story = {
  args: { runtimePort: { connect: () => new Promise(() => {}), status: async () => 'ready' } },
};
export const NarrowLongTranslation: Story = {
  globals: { locale: 'pseudo', viewport: { value: 'mobile' } },
};

export const CatalogUnavailable: Story = { parameters: { catalogUnavailable: true } };
export const CatalogPending: Story = { parameters: { catalogPending: true } };

export const EffectSearchIssues: Story = { parameters: { processorScanIssues: true } };

export const Diagnostics: Story = {
  parameters: { processorScanIssues: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole('button', { name: `${i18n.t('studio.diagnostics')} · 19` }),
    );
    const report = await within(document.body).findByRole('dialog', {
      name: i18n.t('studio.diagnostics'),
    });
    await expect(
      within(report).getByRole('heading', { name: i18n.t('studio.audioEngine') }),
    ).toBeVisible();
    await expect(
      within(report).getByText(i18n.t('routing.processorControl.details', { count: 19 })),
    ).toBeVisible();
  },
};
/** A workspace failure is counted on the status bar and recovered from diagnostics. */
export const WorkspaceProblem: Story = {
  parameters: { layoutUnavailable: true },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await userEvent.click(
      await canvas.findByRole('button', { name: `${i18n.t('studio.diagnostics')} · 1` }),
    );
    const report = await within(document.body).findByRole('dialog', {
      name: i18n.t('studio.diagnostics'),
    });
    await expect(
      await within(report).findByText(i18n.t('routing.layout.failed')),
    ).toBeInTheDocument();
  },
};
export const DiagnosticsNarrow: Story = {
  // A real frame constrains media queries and portalled dialogs as well as the shell.
  render: () => (
    <iframe
      title="Diagnostics at 390 pixels with long translations"
      src="iframe.html?id=app-workspace--diagnostics&viewMode=story&globals=theme:light;locale:pseudo"
      width={390}
      height={844}
    />
  ),
};

const workspaceMidiStatus = {
  ...midiFixture,
  bindings: [
    {
      ...midiBinding,
      target: { graphId: 'graph-1', nodeId: 'trim' },
      graphName: 'Untitled Graph',
      nodeName: 'Monitor trim',
    },
  ],
  targets: [
    {
      target: { graphId: 'graph-1', nodeId: 'trim' },
      label: 'Untitled Graph / Monitor trim',
    },
  ],
};
const workspaceMidiPort = midiPortFixture(workspaceMidiStatus);
export const MidiControls: Story = {
  args: { midiPort: workspaceMidiPort },
  beforeEach: () => workspaceMidiPort.setStatus(workspaceMidiStatus),
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    await canvas.findByRole('group', { name: 'Monitor trim' });
    await userEvent.click(await canvas.findByRole('button', { name: i18n.t('midi.mapMode') }));
    await expect(canvas.getByRole('complementary', { name: i18n.t('midi.title') })).toBeVisible();
    const toggle = canvas.getByRole('button', { name: i18n.t('midi.mapMode') });
    const title = canvas.getByRole('textbox', { name: i18n.t('studio.graphName') });
    await expect(title).toBeDisabled();
    await expect(toggle.closest('header')).toContainElement(title);
    const appearance = canvas.getByRole('button', {
      name: (name) =>
        ['system', 'light', 'dark'].some(
          (theme) => name === i18n.t('theme.current', { theme: i18n.t(`theme.${theme}`) }),
        ),
    });
    await expect(appearance.closest('[inert]')).not.toBeNull();
    await expect(
      canvas.queryByRole('complementary', { name: i18n.t('studio.graphs') }),
    ).not.toBeInTheDocument();
  },
};
