import { StorybookMockControlPort } from '../../core/adapters/fake/StorybookMockControlPort';
import { GRAPH_APPLICATION_TIMEOUT_MS } from '../../core/application/workspace/GraphApplicationQueue';
import { ControlPortError } from '../../core/domain/control/errors';
import type { GraphApplyResponse, RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import type { AudioSessionStatus, FailedProcessorInstance } from '../../core/domain/control/status';
import type { ControlPort } from '../../core/ports/control/ControlPort';
import { ProblemsTarget } from '../../test-support/ProblemsTarget';
import '../../test-support/integration';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { AudioGraphCanvasProps } from '../../core/audio-graph/audioGraphCanvasTypes';
import { GraphApplicationError } from '../../core/domain/graph/WorkspaceEdits';
import { I18nProvider, i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { advance, eventually, find } from '../../test-support/fakeTimerQueries';
import type { RoutingNodeRequest } from './nodeRequests';
import { ProcessorParameterQueue } from './ProcessorParameterQueue';
import { RoutingWorkspace, type RoutingWorkspaceProps } from './RoutingWorkspace';
import { addRoutingNode } from './routingNodeCreation';
import { audioGraphToRuntimeGraph, runtimeGraphToAudioGraph } from './runtimeGraphProjection';
import { virtualPolicy, virtualSpec } from './virtualEndpointFixtures';

const harness = vi.hoisted(() => ({ canvas: null as AudioGraphCanvasProps | null }));
vi.mock('../../core/audio-graph/AudioGraphCanvas', () => ({
  AudioGraphCanvas: (props: AudioGraphCanvasProps) => {
    harness.canvas = props;
    return (
      <section aria-label={props.ariaLabel}>
        {props.model.nodes.map((node) => {
          const controls = props.renderNodeControls?.(node);
          return (
            <div key={node.id}>
              <button type="button">{node.appearance.title}</button>
              {controls?.power}
              {controls?.mute}
              {props.renderNodeContent?.(node)}
            </div>
          );
        })}
      </section>
    );
  },
}));
const empty: RuntimeGraphDocument = { revision: 1, schemaVersion: 2, nodes: [], edges: [] };
function document(...requests: RoutingNodeRequest[]): RuntimeGraphDocument {
  const model = requests.reduce(
    (model, request) => addRoutingNode(model, request, { x: 20, y: 20 }),
    runtimeGraphToAudioGraph(empty),
  );
  return audioGraphToRuntimeGraph(model, empty);
}
const processor: ExternalProcessorDescriptor = {
  typeId: 'vst3:effect',
  displayName: 'Effect',
  vendor: 'Vendor',
  version: '1',
  format: 'vst3',
  hasEditor: false,
  parameters: [],
  defaultLayoutId: 'stereo',
  layouts: [
    {
      layoutId: 'stereo',
      inputs: [{ id: 'main', name: 'Main', kind: 'main', channels: 2, channelFormat: 'stereo' }],
      outputs: [{ id: 'main', name: 'Main', kind: 'main', channels: 2, channelFormat: 'stereo' }],
    },
  ],
};
const signalSource: RoutingNodeRequest = {
  kind: 'audio_input',
  title: 'Hardware input',
  uid: 'device:input',
  channels: 2,
};

function port(graph = empty, failedProcessors: FailedProcessorInstance[] = []) {
  return new StorybookMockControlPort({
    graph,
    graphStatus: {
      protocolVersion: 1,
      activeRevision: graph.revision,
      submittedRevision: graph.revision,
    },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      failedProcessors,
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
    processors: { protocolVersion: 1, processors: [processor] },
  });
}
function view(controlPort: ControlPort, props: Partial<RoutingWorkspaceProps> = {}) {
  return (
    <MixRelayProvider>
      <I18nProvider>
        <ProblemsTarget>
          {(target) => (
            <RoutingWorkspace controlPort={controlPort} problemsTarget={target} {...props} />
          )}
        </ProblemsTarget>
      </I18nProvider>
    </MixRelayProvider>
  );
}
function canvas() {
  if (!harness.canvas) throw new Error('Canvas is not ready');
  return harness.canvas;
}
async function ready() {
  // The first render of a whole screen can exceed waitFor's 1 s default under load.
  await find('region', { name: 'MixRelay virtual I/O routing graph' }, { timeout: 5000 });
  await advance();
}
const session: AudioSessionStatus = {
  format: { sampleRate: 48000, blockFrames: 128 },
  clockAuthority: 'device',
  observedDeviceSampleRate: 48000,
  issue: null,
  transportLatencyFrames: null,
  virtualEndpointSettings: virtualPolicy,
  capabilities: {
    backend: 'core_audio',
    supportedSampleRates: [44100, 48000],
    supportedBlockFrames: [128, 256],
    canReconfigureLive: true,
    hardwareInput: true,
    hardwareOutput: true,
    virtualEndpoint: true,
    applicationCapture: true,
    sampleRateConversion: true,
    driftCorrection: true,
  },
};
beforeEach(async () => {
  harness.canvas = null;
  localStorage.clear();
  await i18n.changeLanguage('en');
});
afterEach(() => vi.useRealTimers());

it('normalizes title edits on blur and commits Enter by leaving the text field', async () => {
  const native = port();
  const title = vi.fn();
  render(view(native, { onTitleChange: title, headerActions: <span>Header action</span> }));
  await ready();
  const input = screen.getByRole('textbox', { name: 'Graph name' });
  fireEvent.change(input, { target: { value: '  Session  ' } });
  expect(title).toHaveBeenLastCalledWith('  Session  ');
  fireEvent.blur(input, { target: { value: '   ' } });
  expect(title).toHaveBeenLastCalledWith('Untitled Graph');
  input.focus();
  fireEvent.keyDown(input, { key: 'Enter' });
  expect(input).not.toHaveFocus();
  expect(screen.getByText('Header action')).toBeInTheDocument();
});

it.each(['compressor', 'limiter', 'processor:vst3:effect'])(
  'routes the authored %s menu selection to one backend graph update',
  async (item) => {
    const native = port();
    const write = vi.spyOn(native, 'applyGraph');
    render(view(native));
    await ready();
    await act(async () => canvas().onAddNode?.(item, { x: 44, y: 33 }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    expect(canvas().model.nodes).toHaveLength(1);
    expect(canvas().model.nodes[0]?.position).toEqual({ x: 44, y: 33 });
  },
);

it.each(['input', 'output'] as const)(
  'adds an actual %s device using its native UID and bounded channels',
  async (direction) => {
    const native = port();
    const write = vi.spyOn(native, 'applyGraph');
    vi.spyOn(native, 'listAudioDevices').mockResolvedValue([
      {
        uid: 'native:device',
        displayName: 'Hardware',
        inputChannels: 6,
        outputChannels: 8,
        isDefaultInput: false,
        isDefaultOutput: false,
      },
    ]);
    render(view(native));
    await ready();
    await act(async () => canvas().onAddNode?.(`audio-${direction}:native:device`, { x: 0, y: 0 }));
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    expect(write.mock.calls[0]?.[0].nodes[0]).toMatchObject({ type: `audio_device_${direction}` });
  },
);

it('captures a discovered application using its exact native selection identity', async () => {
  const native = port();
  const write = vi.spyOn(native, 'applyGraph');
  vi.spyOn(native, 'listCapturableApplications').mockResolvedValue([
    {
      bundleId: 'org.example.other',
      displayName: 'Other',
      iconDataUrl: null,
      processId: 41,
      selector: { match: 'bundle_id', bundle_id: 'org.example.other', include_descendants: true },
    },
    {
      bundleId: 'org.example.player',
      displayName: 'Player',
      iconDataUrl: null,
      processId: 42,
      selector: { match: 'bundle_id', bundle_id: 'org.example.player', include_descendants: true },
    },
  ]);
  render(view(native));
  await ready();
  await act(async () =>
    canvas().onAddNode?.('application:["bundle","org.example.player"]', { x: 0, y: 0 }),
  );
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.calls[0]?.[0].nodes[0]).toMatchObject({ type: 'application_capture' });
});

it.each(['core_audio', 'pipe_wire'] as const)(
  'creates whole-system capture only for %s',
  async (backend) => {
    const native = port();
    const write = vi.spyOn(native, 'applyGraph');
    render(
      view(native, {
        audioSession: { ...session, capabilities: { ...session.capabilities, backend } },
      }),
    );
    await ready();
    await act(async () => canvas().onAddNode?.('system-audio', { x: 0, y: 0 }));
    if (backend === 'pipe_wire') {
      expect(write).not.toHaveBeenCalled();
      return;
    }
    await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    expect(write.mock.calls[0]?.[0].nodes[0]).toMatchObject({
      type: 'application_capture',
      config: {
        selector: {
          match: 'all_except',
          bundle_ids: ['io.mixrelay.desktop', 'io.mixrelay.service'],
        },
        sourcePlayback: 'preserve',
        stream: { channels: 2, channelLayout: 'stereo' },
      },
    });
  },
);

it('ignores an obsolete menu identity and starts native rescan and directory actions without graph mutation', async () => {
  const native = port();
  const write = vi.spyOn(native, 'applyGraph');
  const rescan = vi.spyOn(native, 'rescanProcessors');
  Object.assign(native, { selectProcessorDirectory: vi.fn().mockResolvedValue('/Effects') });
  render(view(native));
  await ready();
  await act(async () => canvas().onAddNode?.('obsolete:selection', { x: 0, y: 0 }));
  await act(async () => canvas().onAddNode?.('rescan-processors', { x: 0, y: 0 }));
  await waitFor(() => expect(rescan).toHaveBeenCalledTimes(1));
  await act(async () => canvas().onAddNode?.('add-processor-directory', { x: 0, y: 0 }));
  await waitFor(() => expect(rescan).toHaveBeenCalledWith(['/Effects']));
  expect(write).not.toHaveBeenCalled();
});

it('renders power and mute against actual node controls, reconciles revision changes, and retains layout history', async () => {
  vi.useFakeTimers();
  const native = port(document(signalSource));
  const write = vi.spyOn(native, 'updateNodeControl');
  const rendered = render(view(native, { graphId: 'graph-1' }));
  await act(async () => vi.advanceTimersByTimeAsync(0));
  expect(screen.getByRole('region', { name: 'MixRelay virtual I/O routing graph' })).toBeVisible();
  const arranged = canvas().model;
  await act(async () =>
    canvas().onModelChange?.(
      {
        ...arranged,
        nodes: arranged.nodes.map((node) => ({ ...node, position: { x: 84, y: 91 } })),
      },
      'layout',
    ),
  );
  // Advance the real controller cadence without waiting for wall-clock polls.
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  const off = screen.getByRole('button', { name: 'Disable Hardware input' });
  expect(off).toBeEnabled();
  await act(async () => fireEvent.click(off));
  expect(write).toHaveBeenCalledTimes(1);
  expect(write.mock.calls[0]?.[2]).toMatchObject({ enabled: false });
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(screen.getByRole('button', { name: 'Enable Hardware input' })).toBeEnabled();
  const mute = screen.getByRole('button', { name: 'Mute Hardware input' });
  expect(mute).toBeEnabled();
  await act(async () => fireEvent.click(mute));
  expect(write).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(screen.getByRole('button', { name: 'Unmute Hardware input' })).toBeEnabled();
  expect(canvas().model.nodes[0]?.position).toEqual({ x: 84, y: 91 });
  rendered.unmount();
  expect(vi.getTimerCount()).toBe(0);
});

it('omits unavailable node controls and disables confirmed buttons while interaction is disabled', async () => {
  vi.useFakeTimers();
  const native = port(document(signalSource));
  const read = vi
    .spyOn(native, 'getNodeControls')
    .mockResolvedValue({ graphId: 'graph-1', revision: 1, nodes: [] });
  const rendered = render(view(native, { graphId: 'graph-1' }));
  await ready();
  expect(screen.queryByRole('button', { name: 'Disable Hardware input' })).not.toBeInTheDocument();
  read.mockRestore();
  rendered.unmount();
  const confirmed = render(view(native, { graphId: 'graph-1' }));
  await ready();
  expect(await find('button', { name: 'Disable Hardware input' })).toBeEnabled();
  confirmed.rerender(view(native, { graphId: 'graph-1', interactionDisabled: true }));
  expect(screen.getByRole('button', { name: 'Disable Hardware input' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Mute Hardware input' })).toBeDisabled();
});

it('targets shared Driver settings without submitting a graph or changing fallback routing names', async () => {
  const native = port(document({ kind: 'virtual_output', spec: virtualSpec }));
  const write = vi.spyOn(native, 'applyGraph');
  const change = vi.fn();
  const rendered = render(
    view(native, {
      audioSession: session,
      deviceDriverAction: {
        settings: virtualPolicy,
        enabled: true,
        error: { endpointId: virtualSpec.id, message: 'Driver settings failed' },
        change,
        create: vi.fn(),
      },
    }),
  );
  await ready();
  const node = canvas().model.nodes[0];
  if (!node) throw new Error('Expected Driver node');
  expect(node.appearance.endpointControl).toMatchObject({
    canRename: true,
    rateError: 'Driver settings failed',
  });
  const update = {
    displayName: 'New name',
    renderChannels: 8,
    captureChannels: 8,
    sampleFormat: 'signed24' as const,
  };
  await act(async () => canvas().onNodeEndpointChange?.(node.id, update));
  expect(change).toHaveBeenCalledExactlyOnceWith(virtualSpec.id, update);
  expect(write).not.toHaveBeenCalled();
  rendered.rerender(
    view(native, {
      audioSession: session,
      deviceDriverAction: { enabled: false, error: null, change, create: vi.fn() },
    }),
  );
  await act(async () => canvas().onNodeEndpointChange?.(node.id, update));
  expect(change).toHaveBeenCalledTimes(1);
  expect(canvas().model.nodes[0]?.appearance.endpointControl).toMatchObject({
    formatsDisabled: true,
    canRename: false,
  });
});

it('changes an endpoint rate only through the enabled Driver action', async () => {
  const native = port(document({ kind: 'virtual_output', spec: virtualSpec }));
  const change = vi.fn();
  const write = vi.spyOn(native, 'applyGraph');
  const driver = { settings: virtualPolicy, error: null, change, create: vi.fn() };
  const rendered = render(
    view(native, { audioSession: session, deviceDriverAction: { ...driver, enabled: true } }),
  );
  await ready();
  const node = canvas().model.nodes[0];
  if (!node) throw new Error('Expected Driver node');
  const update = { displayName: node.appearance.title, renderChannels: 64, captureChannels: 16 };
  await act(async () => canvas().onNodeEndpointChange?.(node.id, { ...update, sampleRate: 48000 }));
  expect(change).toHaveBeenCalledExactlyOnceWith(virtualSpec.id, { ...update, sampleRate: 48000 });
  rendered.rerender(
    view(native, { audioSession: session, deviceDriverAction: { ...driver, enabled: false } }),
  );
  await act(async () => canvas().onNodeEndpointChange?.(node.id, { ...update, sampleRate: 44100 }));
  expect(change).toHaveBeenCalledTimes(1);
  expect(write).not.toHaveBeenCalled();
});

it('retries a layout read failure without a reload choice or audio write', async () => {
  const native = port(document({ kind: 'compressor' }));
  const write = vi.spyOn(native, 'applyGraph');
  const read = vi
    .spyOn(native, 'getGraphLayout')
    .mockRejectedValueOnce(new Error('layout offline'));
  render(view(native, { graphId: 'graph-1' }));
  const retry = await screen.findByRole('button', { name: 'Retry' });
  expect(screen.queryByRole('button', { name: 'Load saved layout' })).not.toBeInTheDocument();
  fireEvent.click(retry);
  await ready();
  expect(read).toHaveBeenCalledTimes(2);
  expect(write).not.toHaveBeenCalled();
});

it('recovers a temporarily unavailable engine with bounded retries and the authoritative graph', async () => {
  vi.useFakeTimers();
  const graph = document({ kind: 'compressor' });
  const native = port(graph);
  const read = vi
    .spyOn(native, 'getGraph')
    .mockRejectedValueOnce(new Error('offline'))
    .mockRejectedValueOnce(new Error('still offline'));
  const rendered = render(view(native));
  await act(async () => Promise.resolve());
  expect(screen.getByText('Engine unavailable')).toBeInTheDocument();
  expect(read).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(499));
  expect(read).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(read).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(500));
  expect(read).toHaveBeenCalledTimes(3);
  expect(canvas().model.nodes).toHaveLength(1);
  expect(screen.queryByText('Engine unavailable')).not.toBeInTheDocument();
  rendered.unmount();
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(read).toHaveBeenCalledTimes(3);
});

it.each(['success', 'failure'] as const)(
  'discards late %s from a previous engine recovery',
  async (outcome) => {
    vi.useFakeTimers();
    const native = port();
    const pending = Promise.withResolvers<RuntimeGraphDocument>();
    vi.spyOn(native, 'getGraph')
      .mockRejectedValueOnce(new Error('offline'))
      .mockReturnValueOnce(pending.promise);
    const rendered = render(view(native));
    await act(async () => Promise.resolve());
    await act(async () => vi.advanceTimersByTimeAsync(500));
    const replacement = port(document({ kind: 'limiter' }));
    rendered.rerender(view(replacement));
    await act(async () => Promise.resolve());
    await act(async () => {
      if (outcome === 'success') pending.resolve(document({ kind: 'compressor' }));
      else pending.reject(new Error('retired engine'));
    });
    expect(canvas().model.nodes[0]?.appearance.title).toBe('Limiter');
    rendered.unmount();
  },
);

it('does not expose signal commands unsupported by the native node capabilities', async () => {
  const graph = document(signalSource);
  const native = port(graph);
  vi.spyOn(native, 'getNodeControls').mockResolvedValue({
    graphId: 'graph-1',
    revision: 1,
    nodes: [
      {
        nodeId: graph.nodes[0]?.id ?? '',
        label: 'Hardware input',
        kind: 'audio_input',
        control: { enabled: true, muted: false },
        capabilities: { power: false, mute: false, recorder: false },
      },
    ],
  });
  render(view(native, { graphId: 'graph-1' }));
  await ready();
  expect(screen.queryByRole('button', { name: 'Disable Hardware input' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Mute Hardware input' })).not.toBeInTheDocument();
});

it('retries only the crashed processor instance through the authoritative graph writer', async () => {
  const graph = document({
    kind: 'provider',
    processor,
    ports: {
      layoutId: 'stereo',
      inputs: processor.layouts[0]?.inputs ?? [],
      outputs: processor.layouts[0]?.outputs ?? [],
    },
  });
  const failed = { graphId: 'graph-1', nodeId: graph.nodes[0]?.id ?? '', instanceId: 'instance-1' };
  const native = port(graph, [failed]);
  const retry = vi.spyOn(native, 'retryProcessor');
  render(view(native, { graphId: 'graph-1', catalogRevision: 1, failedProcessors: [failed] }));
  await ready();
  fireEvent.click(await screen.findByRole('button', { name: 'Retry crashed plug-in: Effect' }));
  await waitFor(() => expect(retry).toHaveBeenCalledTimes(1));
  expect(retry).toHaveBeenCalledWith({
    clientOperationId: expect.any(String),
    graphId: failed.graphId,
    nodeId: failed.nodeId,
    expectedCatalogRevision: 1,
    expectedInstanceId: failed.instanceId,
    expectedRevision: graph.revision,
  });
});

it('offers no endpoint edits without a Driver action and never authors them into the graph', async () => {
  const native = port(document({ kind: 'virtual_output', spec: virtualSpec }));
  render(view(native, { audioSession: { ...session, virtualEndpointSettings: virtualPolicy } }));
  await ready();
  expect(canvas().onNodeEndpointChange).toBeUndefined();
  expect(canvas().model.nodes[0]?.appearance.endpointControl).not.toHaveProperty(
    'sampleRateOptions',
  );
  expect(canvas().model.nodes[0]?.appearance.endpointControl).not.toHaveProperty('canRename');
});

it('saves the current layout after a layout error through the dedicated controller', async () => {
  // Layout saves settle after a 400 ms idle; fake time steps through it.
  vi.useFakeTimers();
  const native = port(document({ kind: 'compressor' }));
  const save = vi
    .spyOn(native, 'setGraphLayout')
    .mockRejectedValueOnce(new Error('layout conflict'));
  const audio = vi.spyOn(native, 'applyGraph');
  render(view(native, { graphId: 'graph-1' }));
  await ready();
  const model = canvas().model;
  await act(async () =>
    canvas().onModelChange?.(
      { ...model, nodes: model.nodes.map((node) => ({ ...node, position: { x: 88, y: 77 } })) },
      'layout',
    ),
  );
  expect(screen.queryByRole('button', { name: 'Load saved layout' })).not.toBeInTheDocument();
  fireEvent.click(await find('button', { name: 'Retry' }));
  await eventually(() => expect(save).toHaveBeenCalledTimes(2));
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  expect(canvas().model.nodes[0]?.position).toEqual({ x: 88, y: 77 });
  expect(audio).not.toHaveBeenCalled();
});

it('publishes parameter Undo and Redo directly and ignores shortcuts owned by text entry', async () => {
  const effect = {
    ...processor,
    parameters: [
      {
        id: 42,
        name: 'Level',
        defaultValue: 0.5,
        canAutomate: true,
        isReadOnly: false,
        stepCount: 0,
      },
    ],
  };
  const layout = effect.layouts[0];
  if (!layout) throw new Error('Expected layout');
  const graph = document({ kind: 'provider', processor: effect, ports: layout });
  const native = port(graph);
  vi.spyOn(native, 'getProcessors').mockResolvedValue({ protocolVersion: 1, processors: [effect] });
  const parameter = vi.spyOn(native, 'setProcessorParameter');
  const audio = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const id = canvas().model.nodes[0]?.id;
  if (!id) throw new Error('Expected effect');
  await act(async () => canvas().onNodeParameterChange?.(id, 'external:42', 0.2));
  await waitFor(() => expect(parameter).toHaveBeenCalledTimes(1));
  await act(async () => Promise.resolve());
  fireEvent.keyDown(screen.getByRole('textbox', { name: 'Graph name' }), {
    key: 'z',
    ctrlKey: true,
  });
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true, altKey: true });
  fireEvent.keyDown(window, { key: 'z' });
  const prevented = new KeyboardEvent('keydown', { key: 'z', ctrlKey: true, cancelable: true });
  prevented.preventDefault();
  fireEvent(window, prevented);
  expect(parameter).toHaveBeenCalledTimes(1);
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  await waitFor(() => expect(parameter).toHaveBeenCalledTimes(2));
  expect(parameter.mock.calls[1]?.[2]).toBe(0.5);
  await act(async () => Promise.resolve());
  fireEvent.keyDown(window, { key: 'y', ctrlKey: true });
  await waitFor(() => expect(parameter).toHaveBeenCalledTimes(3));
  expect(parameter.mock.calls[2]?.[2]).toBe(0.2);
  expect(audio).not.toHaveBeenCalled();
});
it('adopts a playback revision update without discarding authored node positions or history', async () => {
  const graph = document({ kind: 'compressor' });
  const native = port(graph);
  const rendered = render(view(native, { sessionDocument: graph }));
  await ready();
  const model = canvas().model;
  await act(async () =>
    canvas().onModelChange?.(
      { ...model, nodes: model.nodes.map((node) => ({ ...node, position: { x: 70, y: 80 } })) },
      'layout',
    ),
  );
  const update = { ...graph, revision: graph.revision + 1 };
  rendered.rerender(view(native, { sessionDocument: update }));
  await act(async () => Promise.resolve());
  expect(canvas().model.revision).toBe(update.revision);
  expect(canvas().model.nodes[0]?.position).toEqual({ x: 70, y: 80 });
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  expect(canvas().model.nodes[0]?.position).toEqual(model.nodes[0]?.position);
});

it('refreshes changed session documents while preserving existing node geometry and sizing', async () => {
  const first = document({ kind: 'compressor' }),
    next = document({ kind: 'compressor' }, { kind: 'limiter' });
  const unchanged = next.nodes[0];
  if (!unchanged || !first.nodes[0]) throw new Error('shared node required');
  unchanged.id = first.nodes[0].id;
  const native = port(first);
  const rendered = render(view(native, { sessionDocument: first }));
  await ready();
  const model = canvas().model;
  await act(async () =>
    canvas().onModelChange?.(
      {
        ...model,
        nodes: model.nodes.map((node) => ({
          ...node,
          position: { x: 60, y: 65 },
          size: { width: 440, height: 250 },
        })),
      },
      'layout',
    ),
  );
  rendered.rerender(view(native, { sessionDocument: next }));
  await act(async () => Promise.resolve());
  expect(canvas().model.nodes).toHaveLength(2);
  expect(canvas().model.nodes[0]).toMatchObject({
    position: { x: 60, y: 65 },
    size: { width: 440, height: 250 },
  });
  expect(canvas().model.nodes[1]?.position).toEqual(
    runtimeGraphToAudioGraph(next).nodes[1]?.position,
  );
});
it('retains a conflicted draft after a failed restore and retries with new authoritative nodes', async () => {
  const native = port();
  vi.spyOn(native, 'applyGraph').mockRejectedValueOnce(
    new GraphApplicationError('graph_conflict_unresolved'),
  );
  const read = vi.spyOn(native, 'getGraph');
  render(view(native));
  await ready();
  await act(async () => canvas().onAddNode?.('compressor', { x: 110, y: 120 }));
  const reload = await screen.findByRole('button', { name: 'Discard edits and reload engine' });
  const readsBeforeReload = read.mock.calls.length;
  const drain = vi
    .spyOn(ProcessorParameterQueue.prototype, 'whenIdle')
    .mockRejectedValueOnce(new Error('Unexpected drainer failure'));
  fireEvent.click(reload);
  await act(async () => Promise.resolve());
  expect(read).toHaveBeenCalledTimes(readsBeforeReload + 1);
  expect(canvas().model.nodes).toHaveLength(1);
  drain.mockRestore();
  const restored = document({ kind: 'compressor' }, { kind: 'limiter' });
  const recovered = restored.nodes[0];
  const draft = canvas().model.nodes[0];
  if (!recovered || !draft) throw new Error('same conflicted node required');
  recovered.id = draft.id;
  read.mockResolvedValue(restored);
  vi.spyOn(native, 'getGraphStatus').mockResolvedValue({
    protocolVersion: 1,
    activeRevision: restored.revision,
    submittedRevision: restored.revision,
  });
  expect(
    screen.queryByRole('button', { name: 'Discard edits and reload engine' }),
  ).not.toBeInTheDocument();
  const restore = screen.getByRole('button', { name: 'Retry' });
  fireEvent.click(restore);
  await waitFor(() => expect(canvas().model.nodes).toHaveLength(2));
  expect(canvas().model.nodes[0]?.position).toEqual({ x: 110, y: 120 });
  expect(canvas().model.nodes[1]?.position).toEqual(
    runtimeGraphToAudioGraph(restored).nodes[1]?.position,
  );
});
it('shows a bounded graph-apply timeout with the recoverable draft preserved', async () => {
  const native = port();
  const pending = Promise.withResolvers<GraphApplyResponse>();
  vi.spyOn(native, 'applyGraph').mockReturnValueOnce(pending.promise);
  const rendered = render(view(native));
  await ready();
  vi.useFakeTimers();
  await act(async () => canvas().onAddNode?.('limiter', { x: 0, y: 0 }));
  await act(async () => vi.advanceTimersByTimeAsync(GRAPH_APPLICATION_TIMEOUT_MS));
  expect(screen.getByRole('button', { name: 'Retry' })).toBeEnabled();
  expect(
    screen.queryByRole('button', { name: 'Overwrite engine with these edits' }),
  ).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Discard edits and reload engine' }),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent(i18n.t('routing.recovery.timeout'));
  expect(canvas().model.nodes).toHaveLength(1);
  rendered.unmount();
  await act(async () => pending.resolve({ protocolVersion: 1, revision: 2, state: 'active' }));
});

it('rebuilds from authoritative native topology when a node control requires a graph reload', async () => {
  vi.useFakeTimers();
  const initial = document(signalSource);
  const native = port(initial);
  const replacement = document(signalSource, { kind: 'limiter' });
  replacement.nodes = replacement.nodes.map((node, index) =>
    index ? node : { ...node, control: { enabled: false, muted: false } },
  );
  vi.spyOn(native, 'updateNodeControl').mockImplementation(async () => ({
    ...(await native.applyGraph(replacement)),
    requiresGraphReload: true,
  }));
  render(view(native, { graphId: 'graph-1' }));
  await ready();
  fireEvent.click(await find('button', { name: 'Disable Hardware input' }));
  await eventually(() => expect(canvas().model.nodes).toHaveLength(2));
  expect(canvas().model.nodes[0]?.runtime?.control?.enabled).toBe(false);
});

it('treats identical Canvas edits and invalid history shortcuts as no-ops while publishing builtin parameters', async () => {
  const native = port(document({ kind: 'compressor' }));
  const apply = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const current = canvas().model;
  await act(async () => canvas().onModelChange?.(current, 'layout'));
  fireEvent.keyDown(window, { key: 'y', metaKey: true });
  fireEvent.keyDown(window, { key: 'q', ctrlKey: true });
  const id = current.nodes[0]?.id;
  if (!id) throw new Error('Expected compressor');
  await act(async () => canvas().onNodeParameterChange?.(id, 'thresholdDb', -24));
  await waitFor(() => expect(apply).toHaveBeenCalledTimes(1));
  expect(apply.mock.calls[0]?.[0].nodes[0]?.config).toMatchObject({ thresholdDb: -24 });
});
it('ignores a virtual endpoint setting aimed at a node that is no longer present', async () => {
  const native = port();
  const change = vi.fn();
  render(
    view(native, {
      deviceDriverAction: {
        enabled: true,
        change,
        create: vi.fn().mockResolvedValue(null),
        settings: virtualPolicy,
        error: null,
      },
    }),
  );
  await ready();
  await act(async () =>
    canvas().onNodeEndpointChange?.('deleted-node', {
      displayName: 'Gone',
      renderChannels: 2,
      captureChannels: 2,
    }),
  );
  expect(change).not.toHaveBeenCalled();
});

it.each(['same topology', 'changed topology'] as const)(
  'keeps a rejected native node control honest with %s readback',
  async (topology) => {
    vi.useFakeTimers();
    const initial = document(signalSource);
    const replacement =
      topology === 'changed topology' ? document(signalSource, { kind: 'limiter' }) : initial;
    const native = port(initial);
    vi.spyOn(native, 'updateNodeControl').mockRejectedValueOnce(
      new ControlPortError(422, {
        code: 'node_disabled',
        parameters: { nodeControlOutcome: 'not_applied' },
      }),
    );
    // The engine holds `replacement` once the control is rejected. Recovery reads the
    // catalog snapshot, whose document is attributed to the selected graph.
    let engine = initial;
    vi.spyOn(native, 'getGraph').mockImplementation(async () => engine);
    const catalog = await native.getGraphCatalog();
    const read = vi
      .spyOn(native, 'getGraphCatalog')
      .mockImplementation(async () => ({ ...catalog, document: engine }));
    vi.spyOn(native, 'getGraphStatus').mockResolvedValue({
      protocolVersion: 1,
      activeRevision: replacement.revision,
      submittedRevision: replacement.revision,
    });
    render(view(native, { graphId: catalog.activeId }));
    await ready();
    await eventually(() =>
      expect(screen.getByRole('button', { name: 'Disable Hardware input' })).toBeEnabled(),
    );
    const before = read.mock.calls.length;
    engine = replacement;
    fireEvent.click(screen.getByRole('button', { name: 'Disable Hardware input' }));
    await eventually(() => expect(read.mock.calls.length).toBeGreaterThan(before));
    if (topology === 'changed topology') {
      expect(await find('button', { name: 'Retry' })).toBeVisible();
      expect(canvas().model.nodes).toHaveLength(1);
    } else {
      expect(await find('alert')).toBeVisible();
      expect(
        screen.queryByRole('button', { name: 'Overwrite engine with these edits' }),
      ).not.toBeInTheDocument();
    }
  },
);

it('records duplicate completion callbacks for one layout gesture only once', async () => {
  const native = port(document({ kind: 'compressor' }));
  const apply = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const initial = canvas().model;
  const edited = {
    ...initial,
    nodes: initial.nodes.map((node) => ({ ...node, position: { x: 77, y: 88 } })),
  };
  const complete = canvas().onModelChange;
  await act(async () => {
    complete?.(edited, 'layout');
    complete?.(edited, 'layout');
  });
  expect(canvas().model.nodes[0]?.position).toEqual({ x: 77, y: 88 });
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  expect(canvas().model.nodes[0]?.position).toEqual(initial.nodes[0]?.position);
  const undone = canvas().model;
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  expect(canvas().model).toBe(undone);
  expect(apply).not.toHaveBeenCalled();
});

it.each(['read', 'save'])(
  'clears the layout problem once its retry succeeds after a %s failure',
  async (operation) => {
    // Layout saves settle after a 400 ms idle; fake time steps through it.
    vi.useFakeTimers();
    const native = port(document({ kind: 'compressor' }));
    vi.spyOn(
      native,
      operation === 'read' ? 'getGraphLayout' : 'setGraphLayout',
    ).mockRejectedValueOnce(new Error('unknown acknowledgement'));
    render(view(native, { graphId: 'graph-1' }));
    if (operation === 'save') {
      await ready();
      const model = canvas().model;
      await act(async () =>
        canvas().onModelChange?.(
          {
            ...model,
            nodes: model.nodes.map((node) => ({ ...node, position: { x: 88, y: 77 } })),
          },
          'layout',
        ),
      );
    }
    const recovery = await find('button', { name: 'Retry' });
    expect(screen.queryByRole('button', { name: 'Load saved layout' })).not.toBeInTheDocument();
    if (operation === 'read')
      expect(screen.getByText(/node layout could not be loaded/u)).toHaveAttribute(
        'role',
        'status',
      );
    fireEvent.click(recovery);
    await eventually(() =>
      expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument(),
    );
  },
);

it('preserves a user focus move during an asynchronous layout retry', async () => {
  // Layout saves settle after a 400 ms idle; fake time steps through it.
  vi.useFakeTimers();
  const native = port(document({ kind: 'compressor' }));
  vi.spyOn(native, 'setGraphLayout').mockRejectedValueOnce(new Error('unknown acknowledgement'));
  render(
    <>
      <button type="button">Elsewhere</button>
      {view(native, { graphId: 'graph-1' })}
    </>,
  );
  await ready();
  const model = canvas().model;
  await act(async () =>
    canvas().onModelChange?.(
      { ...model, nodes: model.nodes.map((node) => ({ ...node, position: { x: 80, y: 77 } })) },
      'layout',
    ),
  );
  const button = await find('button', { name: 'Retry' });
  const saved = await native.getGraphLayout('graph-1');
  const pending = Promise.withResolvers<typeof saved>();
  vi.spyOn(native, 'getGraphLayout').mockReturnValueOnce(pending.promise);
  button.focus();
  fireEvent.click(button);
  const elsewhere = screen.getByRole('button', { name: 'Elsewhere' });
  elsewhere.focus();
  await act(async () => pending.resolve(saved));
  expect(elsewhere).toHaveFocus();
  expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
});

it('shows microphone consent guidance for an accepted operation and ends promptly on denial', async () => {
  const native = Object.assign(port(), { isOperationPending: vi.fn().mockResolvedValue(true) });
  const pending = Promise.withResolvers<GraphApplyResponse>();
  vi.spyOn(native, 'applyGraph').mockReturnValueOnce(pending.promise);
  vi.spyOn(native, 'getGraphStatus').mockResolvedValue({
    protocolVersion: 1,
    activeRevision: 1,
    submittedRevision: 1,
    pendingReason: 'awaiting_microphone_permission',
  });
  const rendered = render(view(native));
  await ready();
  vi.useFakeTimers();
  await act(async () => canvas().onAddNode?.('limiter', { x: 0, y: 0 }));
  await act(async () => vi.advanceTimersByTimeAsync(20_000));
  expect(screen.getByRole('status')).toHaveTextContent(
    i18n.t('routing.states.awaitingMicrophonePermission'),
  );
  await act(async () =>
    pending.reject(new ControlPortError(403, { code: 'audio_permission_denied', parameters: {} })),
  );
  expect(
    screen.queryByText(i18n.t('routing.states.awaitingMicrophonePermission')),
  ).not.toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent(
    i18n.t('routing.errors.audio_permission_denied'),
  );
  expect(canvas().model.nodes).toHaveLength(1);
  rendered.unmount();
});

// GraphEditSession unit tests cover every owner transition; these check that the workspace
// hands each canvas callback the owner it was created for.
it.each([
  ['parameter', 'loading'],
  ['add', 'active'],
] as const)(
  'discards a previous canvas %s callback when the replacement engine is %s',
  async (action, state) => {
    const native = port(document({ kind: 'effect', effect: 'reverb' }));
    const write = vi.spyOn(native, 'applyGraph');
    const rendered = render(view(native));
    await ready();
    const stale = canvas();
    const nodeId = stale.model.nodes[0]?.id ?? '';
    const replacement = port();
    const pending = Promise.withResolvers<RuntimeGraphDocument>();
    vi.spyOn(replacement, 'getGraph').mockReturnValue(pending.promise);
    const replacementWrite = vi.spyOn(replacement, 'applyGraph');
    rendered.rerender(view(replacement));
    await act(async () => Promise.resolve());
    expect(screen.queryByRole('button', { name: 'Reverb' })).toBeNull();
    if (state === 'active') {
      await act(async () => pending.resolve(empty));
      await ready();
    }
    act(() => {
      if (action === 'add') stale.onAddNode?.('builtin:equalizer', { x: 44, y: 33 });
      else stale.onNodeParameterChange?.(nodeId, 'decaySeconds', 2.3);
    });
    await act(async () => pending.resolve(empty));
    await ready();
    expect(canvas().model.nodes).toEqual([]);
    fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
    await act(async () => Promise.resolve());
    expect(write).not.toHaveBeenCalled();
    expect(replacementWrite).not.toHaveBeenCalled();
  },
);

it('discards a late canvas callback after the workspace unmounts', async () => {
  const native = port(document({ kind: 'effect', effect: 'reverb' }));
  const write = vi.spyOn(native, 'applyGraph');
  const parameter = vi.spyOn(native, 'setProcessorParameter');
  const rendered = render(view(native));
  await ready();
  const stale = canvas();
  rendered.unmount();
  act(() => stale.onNodeParameterChange?.(stale.model.nodes[0]?.id ?? '', 'decaySeconds', 2.3));
  await act(async () => Promise.resolve());
  expect(write).not.toHaveBeenCalled();
  expect(parameter).not.toHaveBeenCalled();
});

it('discards the previous graph owner callback while the current graph edits', async () => {
  const native = port(document({ kind: 'effect', effect: 'reverb' }));
  const write = vi.spyOn(native, 'applyGraph');
  const rendered = render(view(native, { graphId: 'graph-1' }));
  await ready();
  const stale = canvas();
  const catalog = await native.getGraphCatalog();
  const replacement = await native.mutateGraphCatalog({
    action: 'create',
    name: 'Replacement',
    expectedRevision: catalog.document.revision,
    expectedCatalogRevision: catalog.catalogRevision,
  });
  const layout = vi.spyOn(native, 'setGraphLayout');
  rendered.rerender(
    view(native, { graphId: replacement.activeId, sessionDocument: replacement.document }),
  );
  await ready();
  write.mockClear();
  act(() => stale.onNodeParameterChange?.(stale.model.nodes[0]?.id ?? '', 'decaySeconds', 2.3));
  await act(async () => Promise.resolve());
  expect(canvas().model.nodes).toEqual([]);
  fireEvent.keyDown(window, { key: 'z', ctrlKey: true });
  await act(async () => Promise.resolve());
  expect(write).not.toHaveBeenCalled();
  expect(layout).not.toHaveBeenCalled();
  await act(async () => canvas().onAddNode?.('builtin:reverb', { x: 0, y: 0 }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  const nodeId = canvas().model.nodes[0]?.id ?? '';
  await act(async () => canvas().onNodeParameterChange?.(nodeId, 'decaySeconds', 1.3));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(2));
  const written = write.mock.lastCall?.[0];
  expect(written?.nodes[0]?.config).toMatchObject({
    effect: { parameters: { decaySeconds: 1.3 } },
  });
  expect(await native.getGraphCatalog()).toMatchObject({
    activeId: replacement.activeId,
    document: { nodes: written?.nodes },
  });
});

it('presents translated effect names and parameter labels after a language change', async () => {
  const native = port(document({ kind: 'effect', effect: 'equalizer' }));
  const write = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const bands = () =>
    canvas().model.nodes[0]?.appearance.equalizer?.bands.map((band) => band.label);
  const english = bands();
  await act(async () => i18n.changeLanguage('ko-KR'));
  expect(bands()).toEqual(['lowDb', 'midDb', 'highDb'].map((key) => i18n.t(`effects.${key}`)));
  expect(bands()).not.toEqual(english);
  await act(async () => canvas().onAddNode?.('builtin:equalizer', { x: 0, y: 0 }));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.lastCall?.[0].nodes[1]).toMatchObject({ label: i18n.t('effects.equalizer') });
});

it('keeps the canvas mounted and focus inside it while an edit applies', async () => {
  const native = port(document({ kind: 'effect', effect: 'channel_utility' }));
  const write = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const node = canvas().model.nodes[0];
  if (!node) throw new Error('Expected channel utility');
  const focused = screen.getByRole('button', { name: node.appearance.title });
  act(() => focused.focus());
  await act(async () => canvas().onNodeParameterChange?.(node.id, 'mode', 2));
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(screen.getByRole('button', { name: node.appearance.title })).toBe(focused);
  expect(focused).toHaveFocus();
});

it('applies every value of a fast gesture that arrives before the next render', async () => {
  const native = port(document({ kind: 'effect', effect: 'equalizer' }));
  const write = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const stale = canvas();
  const id = stale.model.nodes[0]?.id ?? '';
  act(() => {
    for (const value of [0.708, 0.709, 0.71]) stale.onNodeParameterChange?.(id, 'midQ', value);
  });
  await waitFor(() =>
    expect(write.mock.lastCall?.[0].nodes[0]?.config).toMatchObject({
      effect: { parameters: { midQ: 0.71 } },
    }),
  );
});

it('applies both values of an EQ band gesture in one graph update', async () => {
  const native = port(document({ kind: 'effect', effect: 'equalizer' }));
  const write = vi.spyOn(native, 'applyGraph');
  render(view(native));
  await ready();
  const id = canvas().model.nodes[0]?.id ?? '';
  await act(async () =>
    canvas().onNodeParametersChange?.(id, [
      { id: 'midFrequencyHz', value: 1500 },
      { id: 'midDb', value: 3 },
    ]),
  );
  await waitFor(() => expect(write).toHaveBeenCalledTimes(1));
  expect(write.mock.lastCall?.[0].nodes[0]?.config).toMatchObject({
    effect: { parameters: { midFrequencyHz: 1500, midDb: 3, lowFrequencyHz: 150 } },
  });
});

it('reads externally changed effect settings automatically, then edits those current values', async () => {
  vi.useFakeTimers();
  const native = port(document({ kind: 'effect', effect: 'reverb' }));
  const write = vi.spyOn(native, 'applyGraph');
  render(view(native, { graphId: 'graph-1' }));
  await ready();
  const engine = await native.getGraph();
  const current = {
    ...engine,
    revision: engine.revision + 1,
    nodes: engine.nodes.map((node) => ({
      ...node,
      config: {
        ...(node.config as object),
        effect: {
          kind: 'reverb',
          parameters: {
            decaySeconds: 3,
            damping: 0.6,
            mix: 0.4,
            algorithm: 0,
            roomSize: 1,
            preDelayMs: 0,
          },
        },
      },
    })),
  };
  await act(async () => {
    await native.applyGraph(current);
  });
  const damping = () =>
    canvas().model.nodes[0]?.appearance.parameterControls?.find(
      (control) => control.id === 'damping',
    )?.value;
  await eventually(() => expect(damping()).toBe(0.6), { timeout: 2500 });
  expect(
    screen.queryByRole('button', { name: 'Overwrite engine with these edits' }),
  ).not.toBeInTheDocument();
  const id = canvas().model.nodes[0]?.id ?? '';
  await act(async () => canvas().onNodeParameterChange?.(id, 'damping', 0.7));
  await eventually(() =>
    expect(write.mock.lastCall?.[0]).toMatchObject({
      revision: current.revision + 1,
      nodes: [{ config: { effect: { parameters: { damping: 0.7, mix: 0.4, decaySeconds: 3 } } } }],
    }),
  );
});
