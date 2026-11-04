import type { Meta, StoryObj } from '@storybook/react-vite';
import { useMemo, useState } from 'react';
import { expect, userEvent, waitFor, within } from 'storybook/test';
import mixrelayAppIcon from '../../../../desktop-shell/src-tauri/icons/32x32.png';
import { processorReportState } from '../../../.storybook/fixtures/processorReport';
import {
  type StorybookControlState,
  StorybookMockControlPort,
} from '../../core/adapters/fake/StorybookMockControlPort';
import { ControlPortError } from '../../core/domain/control/errors';
import type { GraphApplyResponse, RuntimeGraphDocument } from '../../core/domain/control/graph';
import { i18n } from '../../core/i18n';
import { ActionButton } from '../../core/ui/ActionButton';
import { ContentSection } from '../../core/ui/ContentSection';
import { renderRecorderNode } from '../../screens/recorderNodeView';
import type { ChainKind } from './builtinEffects';
import { addRoutingNode, audioGraphToRuntimeGraph } from './model';
import { RoutingWorkspace, type RoutingWorkspaceProps } from './RoutingWorkspace';
import { deviceActivityFixture, deviceActivitySession } from './virtualEndpointFixtures';

const graph: RuntimeGraphDocument = {
  edges: [],
  nodes: [
    {
      config: {
        reference: { render: true, capture: true },
        capture: { channelLayout: 'stereo', channels: 2 },
        defaultSampleRate: 48_000,
        displayName: 'MixRelay Virtual I/O',
        id: 'default-virtual-io',
        render: { channelLayout: 'stereo', channels: 2 },
        supportedSampleRates: [48_000],
      },
      id: 'default-virtual-io',
      label: 'MixRelay Virtual I/O',
      type: 'virtual_endpoint',
    },
  ],
  revision: 1,
  schemaVersion: 2,
};

const controlState = {
  graph,
  graphStatus: { activeRevision: 1, protocolVersion: 1, submittedRevision: 1 },
  meters: {
    nodes: [{ nodeId: 'default-virtual-io', peaks: [0.42, 0.19], portId: 'from_os' }],
    protocolVersion: 1,
    revision: 1,
  },
  status: {
    bufferFrames: 128,
    driverState: 'ready',
    overruns: 0,
    protocolVersion: 1,
    sampleRate: 48_000,
    underruns: 0,
  },
} satisfies StorybookControlState;

const applicationGraph: RuntimeGraphDocument = {
  ...graph,
  edges: [
    {
      channelMapping: { mode: 'auto' },
      destination: { nodeId: 'gain-1', portId: 'audio' },
      id: 'parsec-to-gain',
      source: { nodeId: 'application-1', portId: 'audio' },
    },
    {
      channelMapping: { mode: 'auto' },
      destination: { nodeId: 'gain-1', portId: 'audio' },
      id: 'safari-to-gain',
      source: { nodeId: 'application-2', portId: 'audio' },
    },
    {
      channelMapping: { mode: 'auto' },
      destination: { nodeId: 'default-virtual-io', portId: 'to_os' },
      id: 'gain-to-virtual',
      source: { nodeId: 'gain-1', portId: 'audio' },
    },
  ],
  nodes: [
    ...graph.nodes,
    {
      config: {
        gainDb: -3,
        selector: {
          bundle_id: 'tv.parsec.www',
          include_descendants: true,
          match: 'bundle_id',
        },
        sourcePlayback: 'preserve',
        stream: { channelLayout: 'stereo', channels: 2 },
      },
      id: 'application-1',
      label: 'Parsec',
      type: 'application_capture',
    },
    {
      config: {
        gainDb: -6,
        selector: {
          bundle_id: 'com.apple.Safari',
          include_descendants: true,
          match: 'bundle_id',
        },
        sourcePlayback: 'preserve',
        stream: { channelLayout: 'stereo', channels: 2 },
      },
      id: 'application-2',
      label: 'Safari',
      type: 'application_capture',
    },
    {
      config: { channels: 2, gainDb: -6 },
      id: 'gain-1',
      label: 'Stream trim',
      type: 'gain',
    },
  ],
  revision: 2,
};

const applicationControlState = {
  graph: applicationGraph,
  graphStatus: { activeRevision: 2, protocolVersion: 1, submittedRevision: 2 },
  meters: {
    nodes: [
      { nodeId: 'application-1', peaks: [0.61, 0.48], portId: 'audio' },
      { nodeId: 'application-2', peaks: [0.37, 0.29], portId: 'audio' },
      { nodeId: 'gain-1', peaks: [0.31, 0.24], portId: 'audio' },
      { nodeId: 'default-virtual-io', peaks: [0.31, 0.24], portId: 'to_os' },
    ],
    protocolVersion: 1,
    revision: 2,
  },
  status: {
    bufferFrames: 128,
    driverState: 'ready',
    overruns: 0,
    protocolVersion: 1,
    sampleRate: 48_000,
    underruns: 0,
  },
} satisfies StorybookControlState;

const meta = {
  title: 'Features/Routing/Workspace',
  component: RoutingWorkspace,
  args: { graphId: 'graph-1', renderRecorder: renderRecorderNode },
  loaders: [
    ({ parameters }) => ({
      controlPort: parameters.graphSubmissionState
        ? graphSubmissionFixture(parameters.graphSubmissionState)
        : parameters.graphReadState
          ? graphReadFixture(parameters.graphReadState)
          : parameters.chain
            ? chainPort(parameters.chain, parameters.savedControl, parameters.commandDelay)
            : new StorybookMockControlPort(
                structuredClone(
                  parameters.applicationGraph ? applicationControlState : controlState,
                ),
              ),
    }),
  ],
  render: (args, { loaded }) => (
    <WorkspaceWithProblems {...args} controlPort={loaded.controlPort} />
  ),
  decorators: [
    (Story) => (
      <div style={{ height: '100dvh', minHeight: 320 }}>
        <Story />
      </div>
    ),
  ],
  parameters: { layout: 'fullscreen' },
} satisfies Meta<typeof RoutingWorkspace>;

export default meta;

/** In the app, workspace problems render in the diagnostics window; here they sit below. */
function WorkspaceWithProblems(props: RoutingWorkspaceProps) {
  const [target, setTarget] = useState<HTMLDivElement | null>(null);
  const [count, setCount] = useState(0);
  return (
    <div style={{ display: 'grid', gridTemplateRows: 'minmax(0, 1fr) auto', height: '100%' }}>
      <RoutingWorkspace
        {...props}
        renderRecorder={renderRecorderNode}
        problemsTarget={target}
        onProblemsChange={(summary) => setCount(summary.codes.length)}
      />
      <div hidden={!count}>
        <ContentSection title={i18n.t('studio.currentProblems')} divider="before">
          <div ref={setTarget} />
        </ContentSection>
      </div>
    </div>
  );
}
// The loader owns a fresh control port for every story selection.
type Story = StoryObj<Partial<RoutingWorkspaceProps>>;

export const VirtualIoPassThrough: Story = {};
export const ApplicationToVirtualInput: Story = {
  parameters: { applicationGraph: true },
};
export const WholeSystemCapture: Story = {
  args: { audioSession: deviceActivitySession },
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const surface = within(canvasElement.ownerDocument.body);
    await userEvent.click(await canvas.findByRole('button', { name: i18n.t('routing.addNode') }));
    await userEvent.hover(
      await surface.findByRole('menuitem', { name: i18n.t('routing.groups.source') }),
    );
    await userEvent.click(
      await surface.findByRole('menuitem', { name: i18n.t('studioGraph.nodes.systemAudio.name') }),
    );
    await expect(
      canvas.findByText(i18n.t('studioGraph.nodes.systemAudio.name')),
    ).resolves.toBeVisible();
  },
};
export const WholeSystemCaptureNarrow: Story = {
  ...WholeSystemCapture,
  globals: { viewport: { value: 'mobile' }, locale: 'pseudo' },
};
export const MobileNarrow: Story = {
  globals: { viewport: { value: 'mobile' } },
};
export const LongTranslation: Story = { globals: { locale: 'pseudo' } };

function graphReadFixture(state: 'loading' | 'unavailable') {
  const port = new StorybookMockControlPort(structuredClone(controlState));
  port.getGraph =
    state === 'loading'
      ? () => new Promise<RuntimeGraphDocument>(() => {})
      : async () => {
          throw new Error('Storybook graph read unavailable');
        };
  return port;
}

export const Loading: Story = { parameters: { graphReadState: 'loading' } };
export const Unavailable: Story = { parameters: { graphReadState: 'unavailable' } };
export const EditingLocked: Story = {
  args: { interactionDisabled: true },
  parameters: {
    applicationGraph: true,
    docs: {
      description: {
        story:
          '처음부터 interactionDisabled=true로 열어 노드 제어값 조회를 제한합니다. 미확인 Off/Mute 조작은 표시하지 않으며 그래프·미터는 유지합니다. 확인 후 제한되는 disabled 전환은 계약 검사에서 다룹니다. 전체 입력 차단은 상위 화면의 inert가 소유합니다.',
      },
    },
  },
};

function graphSubmissionFixture(mode: 'pending' | 'permission' | 'recovery' | 'automatic_merge') {
  const state: StorybookControlState = structuredClone(controlState);
  const port = Object.assign(new StorybookMockControlPort(state), {
    isOperationPending: async () => mode === 'permission',
  });
  const apply = port.applyGraph.bind(port);
  let firstRequest = true;
  let externalUpdates = 0;
  port.applyGraph = async (request) => {
    if (mode === 'permission') {
      state.graphStatus.pendingReason = 'awaiting_microphone_permission';
      return new Promise<GraphApplyResponse>(() => {});
    }
    if (mode === 'pending') return new Promise<GraphApplyResponse>(() => {});
    if (mode === 'automatic_merge' && externalUpdates < 2) {
      externalUpdates += 1;
      state.graph = {
        ...state.graph,
        revision: state.graph.revision + 1,
        nodes: [
          ...state.graph.nodes,
          {
            id: `external-trim-${externalUpdates}`,
            label: `External trim ${externalUpdates}`,
            type: 'gain',
            config: { channels: 2, gainDb: -6 },
          },
        ],
      };
      state.graphStatus = {
        protocolVersion: 1,
        activeRevision: state.graph.revision,
        submittedRevision: state.graph.revision,
      };
      throw new ControlPortError(409, { code: 'graph_revision_stale', parameters: {} });
    }
    if (mode === 'recovery' && firstRequest) {
      firstRequest = false;
      throw new ControlPortError(503, { code: 'control_unavailable', parameters: {} });
    }
    return apply(request);
  };
  return port;
}

async function addRecorder(canvasElement: HTMLElement) {
  const canvas = within(canvasElement);
  const surface = within(canvasElement.ownerDocument.body);
  await userEvent.click(await canvas.findByRole('button', { name: i18n.t('routing.addNode') }));
  await userEvent.hover(
    await surface.findByRole('menuitem', { name: i18n.t('routing.groups.output') }),
  );
  await userEvent.click(await surface.findByRole('menuitem', { name: i18n.t('recorder.title') }));
}

export const SubmissionPending: Story = {
  parameters: {
    graphSubmissionState: 'pending',
    docs: {
      description: {
        story:
          '실제 Add node에서 녹음기를 추가하고 applyGraph 응답을 대기합니다. 제품의 10초 요청 상한이 지나면 timeout 복구 상태로 전환합니다. 영구 loading fixture가 아닙니다.',
      },
    },
  },
  play: async ({ canvasElement }) => {
    await addRecorder(canvasElement);
    await waitFor(() => {
      expect(canvasElement.querySelector('section[data-state="applying"]')).toBeVisible();
    });
  },
};

export const MicrophonePermissionWaiting: Story = {
  parameters: { graphSubmissionState: 'permission' },
  play: async ({ canvasElement }) => {
    await addRecorder(canvasElement);
    await expect(
      await within(canvasElement).findByText(i18n.t('routing.states.awaitingMicrophonePermission')),
    ).toBeVisible();
  },
};

export const SubmissionRecovery: Story = {
  parameters: {
    graphSubmissionState: 'recovery',
    docs: {
      description: {
        story:
          '첫 제출 응답이503으로 실패합니다. 불확실한 명령을 자동반복하지 않고 초안·오류와 다시 시도 하나를 유지합니다. 명시적재시도는최신문서와자동병합합니다. 덮어쓰기/편집취소를묻지않으며 오디오·파일은변경하지않습니다.',
      },
    },
  },
  play: async ({ canvasElement }) => {
    await addRecorder(canvasElement);
    await expect(
      await within(canvasElement).findByRole('button', { name: i18n.t('routing.inventory.retry') }),
    ).toBeVisible();
    await expect(
      await within(canvasElement).findByText(i18n.t('routing.recovery.draftPreserved')),
    ).toBeVisible();
    await expect(
      within(canvasElement).queryByRole('button', { name: i18n.t('routing.recovery.retry') }),
    ).not.toBeInTheDocument();
    await expect(
      within(canvasElement).queryByRole('button', { name: i18n.t('routing.recovery.reload') }),
    ).not.toBeInTheDocument();
  },
};

export const NarrowSubmissionRecovery: Story = {
  ...SubmissionRecovery,
  globals: { locale: 'pseudo', viewport: { value: 'mobile' } },
};

export const AutomaticRevisionMerge: Story = {
  parameters: {
    graphSubmissionState: 'automatic_merge',
    docs: {
      description: {
        story:
          '녹음기 추가 중 외부 작성자가 서로 다른 trim 노드를 두 번 추가합니다. 반복 revision 확정 거부를 자동 재병합하여 녹음기와 외부 변경 둘을 보존합니다. 그래프 선택·덮어쓰기·편집 취소를 요구하지 않습니다.',
      },
    },
  },
  play: async ({ canvasElement, loaded }) => {
    await addRecorder(canvasElement);
    await waitFor(async () => {
      const document: RuntimeGraphDocument = await loaded.controlPort.getGraph();
      expect(document.nodes.some((node) => node.type === 'recorder')).toBe(true);
      expect(document.nodes.map((node) => node.id)).toEqual(
        expect.arrayContaining(['external-trim-1', 'external-trim-2']),
      );
      expect(canvasElement.querySelector('section[data-state="active"]')).toBeVisible();
      expect(
        within(canvasElement).queryByRole('button', { name: i18n.t('routing.recovery.retry') }),
      ).not.toBeInTheDocument();
      expect(
        within(canvasElement).queryByRole('button', { name: i18n.t('routing.recovery.reload') }),
      ).not.toBeInTheDocument();
    });
  },
};

function chainPort(
  preset: ChainKind,
  savedControl?: { enabled: boolean; muted: boolean },
  commandDelay = 0,
) {
  const empty = {
    schemaVersion: 2,
    revision: 1,
    nodes: [],
    edges: [],
  } satisfies RuntimeGraphDocument;
  const model = addRoutingNode(
    { revision: 1, nodes: [], edges: [] },
    { kind: 'effect_chain', preset },
    { x: 0, y: 0 },
  );
  const runtime = audioGraphToRuntimeGraph(model, empty);
  const port = new StorybookMockControlPort({
    layout: { positions: Object.fromEntries(model.nodes.map((node) => [node.id, node.position])) },
    graph: {
      ...runtime,
      nodes: runtime.nodes.map((node, index) =>
        savedControl && index === 0 ? { ...node, control: savedControl } : node,
      ),
    },
    graphStatus: {
      activeRevision: model.revision,
      submittedRevision: model.revision,
      protocolVersion: 1,
    },
    status: {
      bufferFrames: 128,
      driverState: 'ready',
      overruns: 0,
      protocolVersion: 1,
      sampleRate: 48_000,
      underruns: 0,
    },
  });
  if (commandDelay) {
    const update = port.updateNodeControl.bind(port);
    port.updateNodeControl = async (...args) => {
      await new Promise((resolve) => setTimeout(resolve, commandDelay));
      return update(...args);
    };
  }
  return port;
}
export const KaraokeChain: Story = {
  parameters: { chain: 'karaoke' },
};
export const TunnelChain: Story = {
  parameters: { chain: 'tunnel' },
};
export const FishbowlChain: Story = {
  parameters: { chain: 'fishbowl' },
};
export const RadioChain: Story = {
  parameters: { chain: 'radio' },
};
export const SavedEffectOffAndMute: Story = {
  parameters: { chain: 'tunnel', savedControl: { enabled: false, muted: true } },
};
export const RecoveryKeyboard: Story = {
  globals: { locale: 'en' },
  parameters: { chain: 'tunnel', savedControl: { enabled: false, muted: true }, commandDelay: 500 },
  play: async ({ canvasElement }) => {
    const surface = within(canvasElement);
    const enable = (await surface.findAllByRole('button', { name: /^Enable / }))[0];
    const group = enable?.closest('[role="group"]');
    if (!enable || !group) throw new Error('Missing saved recovery owner');
    const card = within(group as HTMLElement);
    enable.focus();
    await userEvent.keyboard('{Enter}');
    await expect(enable).toBeInTheDocument();
    await expect(enable).toBeDisabled();
    const unmute = await card.findByRole('button', { name: /^Unmute / });
    await waitFor(() => expect(unmute).toHaveFocus());
    await userEvent.keyboard(' ');
    await expect(unmute).toBeDisabled();
    const bypass = card.getByRole('button', { name: /: Bypass:/ });
    await waitFor(() => expect(bypass).toHaveFocus());
    await expect(bypass).toBeEnabled();
    await expect(card.queryByRole('button', { name: /^Enable |^Unmute / })).not.toBeInTheDocument();
  },
};

function ExternalSettingsDemo() {
  const port = useMemo(() => chainPort('tunnel'), []);
  const update = async () => {
    const document = await port.getGraph();
    await port.applyGraph({
      ...document,
      revision: document.revision + 1,
      nodes: document.nodes.map((node) => {
        const config = node.config as {
          effect?: { kind: string; parameters: Record<string, number> };
        };
        if (config.effect?.kind !== 'reverb') return node;
        const parameters = config.effect.parameters;
        return {
          ...node,
          config: {
            ...config,
            effect: {
              ...config.effect,
              parameters: {
                ...parameters,
                decaySeconds: parameters.decaySeconds === 4 ? 2 : 4,
                damping: 0.65,
                mix: 0.4,
              },
            },
          },
        };
      }),
    });
  };
  return (
    <>
      <ActionButton onClick={() => void update()}>Change engine settings</ActionButton>
      <WorkspaceWithProblems controlPort={port} graphId="graph-1" />
    </>
  );
}
export const ExternalSettings: Story = { render: () => <ExternalSettingsDemo /> };

function ExternalLayoutDemo() {
  const port = useMemo(() => {
    const fixture = chainPort('tunnel');
    const write = fixture.setGraphLayout.bind(fixture);
    fixture.setGraphLayout = async (id, request) => {
      try {
        return await write(id, request);
      } catch (error) {
        if (error instanceof Error && error.message === 'graph_revision_stale')
          throw new ControlPortError(409, {
            code: 'graph_revision_stale',
            parameters: { resource: 'graph_layout', graphId: id },
          });
        throw error;
      }
    };
    return fixture;
  }, []);
  const change = async () => {
    const saved = await port.getGraphLayout('graph-1');
    await port.setGraphLayout('graph-1', {
      expectedLayoutRevision: saved.layoutRevision,
      layout: { ...saved.layout, viewport: { x: 60, y: 40, zoom: 0.75 } },
    });
  };
  return (
    <>
      <ActionButton onClick={() => void change()}>
        Change saved layout in another window
      </ActionButton>
      <WorkspaceWithProblems controlPort={port} graphId="graph-1" />
    </>
  );
}
export const ExternalLayoutRevision: Story = { render: () => <ExternalLayoutDemo /> };

export const DeviceApps: Story = {
  args: { audioSession: deviceActivitySession },
  loaders: [() => ({ controlPort: deviceActivityPort('observed') })],
};
export const DeviceAppsEmpty: Story = {
  ...DeviceApps,
  loaders: [() => ({ controlPort: deviceActivityPort('empty') })],
};
export const DeviceAppsUnavailable: Story = {
  ...DeviceApps,
  loaders: [() => ({ controlPort: deviceActivityPort('unavailable') })],
};
export const DeviceAppsChecking: Story = {
  ...DeviceApps,
  loaders: [() => ({ controlPort: deviceActivityPort('loading') })],
};
export const DeviceAppsNarrowPseudo: Story = {
  ...DeviceApps,
  globals: { locale: 'pseudo', theme: 'dark' },
  parameters: {
    viewport: {
      defaultViewport: 'mobile',
      options: { mobile: { name: '390px', styles: { width: '390px', height: '844px' } } },
    },
  },
};

function deviceActivityPort(state: 'observed' | 'empty' | 'unavailable' | 'loading') {
  const fixture = Object.fromEntries(
    Object.entries(structuredClone(deviceActivityFixture)).map(([uid, records]) => [
      uid,
      records.filter((app) => app.processId !== 51),
    ]),
  );
  for (const records of Object.values(fixture))
    for (const app of records) {
      if (app.bundleId === 'io.mixrelay.desktop') app.iconDataUrl = mixrelayAppIcon;
    }
  const port = new StorybookMockControlPort({
    ...structuredClone(controlState),
    deviceAppActivity: state === 'empty' ? {} : fixture,
  });
  if (state === 'unavailable')
    port.getVirtualDeviceAppActivity = () => Promise.reject(new Error('unavailable'));
  if (state === 'loading') port.getVirtualDeviceAppActivity = () => new Promise(() => {});
  return port;
}

export const EffectSearchIssues: Story = {
  loaders: [() => ({ controlPort: new StorybookMockControlPort(processorReportState()) })],
};
export const EffectSearchReport: Story = {
  ...EffectSearchIssues,
  play: async ({ canvasElement }) => {
    const surface = within(canvasElement.ownerDocument.body);
    await userEvent.click(await surface.findByRole('button', { name: i18n.t('routing.addNode') }));
    await userEvent.hover(
      await surface.findByRole('menuitem', { name: i18n.t('routing.groups.processor') }),
    );
    await userEvent.click(
      await surface.findByRole('menuitem', { name: i18n.t('routing.processorControl.viewReport') }),
    );
    await expect(
      surface.findByRole('dialog', { name: i18n.t('routing.processorControl.reportTitle') }),
    ).resolves.toBeVisible();
  },
};
export const EffectSearchReportNarrow: Story = {
  ...EffectSearchReport,
  globals: { viewport: { value: 'mobile' }, locale: 'pseudo' },
  parameters: {
    viewport: {
      defaultViewport: 'mobile',
      options: { mobile: { name: '390px', styles: { width: '390px', height: '844px' } } },
    },
  },
};
export const FailedPluginNode: Story = {
  args: {
    catalogRevision: 1,
    failedProcessors: [{ graphId: 'graph-1', nodeId: 'voice', instanceId: 'failed-instance' }],
  },
  loaders: [
    () => ({ controlPort: new StorybookMockControlPort(processorReportState({ failed: true })) }),
  ],
};
export const PluginEditorIssue: Story = {
  loaders: [
    async () => {
      const controlPort = new StorybookMockControlPort(processorReportState({ editor: true }));
      await controlPort.setGraphLayout('graph-1', {
        expectedLayoutRevision: 0,
        layout: {
          positions: { voice: { x: 80, y: 80 }, music: { x: 420, y: 80 } },
          viewport: { x: 0, y: 0, zoom: 1 },
        },
      });
      controlPort.getProcessorEditor = async (nodeId) => {
        if (nodeId === 'voice') throw new Error('Editor status unavailable');
        return {
          protocolVersion: 1,
          nodeId,
          state: 'open',
          failure: null,
          width: null,
          height: null,
          resizable: false,
        };
      };
      return { controlPort };
    },
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const node = await canvas.findByRole('group', { name: 'Voice effect' });
    const retry = await within(node).findByRole('button', {
      name: i18n.t('routing.processorControl.retry'),
    });
    await waitFor(() =>
      expect(retry.getBoundingClientRect().bottom).toBeLessThanOrEqual(
        node.getBoundingClientRect().bottom,
      ),
    );
  },
};

export const SequentialInsertions: Story = {
  loaders: [
    () => ({
      controlPort: new StorybookMockControlPort({
        ...structuredClone(controlState),
        graph: { ...graph, nodes: [] },
      }),
    }),
  ],
  play: async ({ canvasElement }) => {
    const canvas = within(canvasElement);
    const body = within(canvasElement.ownerDocument.body);
    await userEvent.click(await canvas.findByRole('button', { name: i18n.t('routing.addNode') }));
    await userEvent.hover(
      await body.findByRole('menuitem', { name: i18n.t('routing.groups.processor') }),
    );
    await userEvent.click(
      await body.findByRole('menuitem', { name: i18n.t('routing.nodes.compressor') }),
    );
    const compressor = await canvas.findByRole('group', {
      name: i18n.t('routing.nodes.compressor'),
    });
    await addRecorder(canvasElement);
    const recorder = await canvas.findByRole('group', { name: i18n.t('recorder.title') });
    await waitFor(() => {
      const a = compressor.getBoundingClientRect();
      const b = recorder.getBoundingClientRect();
      expect(a.right <= b.left || b.right <= a.left || a.bottom <= b.top || b.bottom <= a.top).toBe(
        true,
      );
    });
    compressor.focus();
    await userEvent.keyboard('{Shift>}{F10}{/Shift}');
    await expect(
      body.findByRole('menuitem', { name: i18n.t('routing.nodeName.title') }),
    ).resolves.toBeVisible();
  },
};
