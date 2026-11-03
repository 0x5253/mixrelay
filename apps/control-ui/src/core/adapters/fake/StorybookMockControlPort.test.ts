import { assert, describe, expect, it } from 'vitest';
import type { ProcessorRetryRequest, RuntimeGraphDocument } from '../../domain/control/graph';
import { type StorybookControlState, StorybookMockControlPort } from './StorybookMockControlPort';

function fixture(graph?: RuntimeGraphDocument) {
  const state: StorybookControlState = {
    graph: graph ?? {
      schemaVersion: 2,
      revision: 1,
      edges: [],
      nodes: [
        { id: 'source', type: 'application_capture', label: 'Firefox', config: null },
        { id: 'fx', type: 'provider', label: 'Effect', config: { typeId: 'vst3:fx' } },
        { id: 'sink', type: 'virtual_endpoint', label: 'Device', config: { gainDb: -3 } },
      ],
    },
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  };
  return { state, port: new StorybookMockControlPort(state) };
}

async function catalogMutation(
  port: StorybookMockControlPort,
  action: 'create' | 'select' | 'rename' | 'delete',
  id = 'graph-1',
) {
  const catalog = await port.getGraphCatalog();
  const fence = {
    expectedCatalogRevision: catalog.catalogRevision,
    expectedRevision: catalog.document.revision,
  };
  if (action === 'create') return port.mutateGraphCatalog({ ...fence, action, name: 'New graph' });
  if (action === 'rename')
    return port.mutateGraphCatalog({ ...fence, action, id, name: 'Renamed graph' });
  return port.mutateGraphCatalog({ ...fence, action, id });
}

describe('Storybook control preview boundaries', () => {
  it('restores independent initial node geometry and saves edits without changing audio revision', async () => {
    const { state } = fixture();
    const layout = { positions: { fx: { x: 260, y: 0 } } };
    const port = new StorybookMockControlPort({ ...state, layout });
    layout.positions.fx.x = 999;
    const restored = await port.getGraphLayout('graph-1');
    expect(restored.layout.positions.fx?.x).toBe(260);
    const saved = await port.setGraphLayout('graph-1', {
      expectedLayoutRevision: 0,
      layout: { positions: { fx: { x: 520, y: 0 } } },
    });
    expect(saved.layoutRevision).toBe(1);
    expect((await port.getGraphLayout('graph-1')).layout.positions.fx?.x).toBe(520);
    expect((await port.getGraph()).revision).toBe(state.graph.revision);
  });
  it('isolates recorder preferences by graph and recorder and never presents a recorded take', async () => {
    const { port } = fixture();
    expect(await port.recordingSettings('graph-1', 'take-1')).toEqual({
      folder: '/Desktop',
      sampleRate: null,
      encoding: 'pcm16',
    });
    const settings = { folder: '/Takes', sampleRate: 96000, encoding: 'pcm24' as const };
    const saved = await port.setRecordingSettings('graph-1', 'take-1', settings, 1);
    saved.folder = '/modified';
    expect(await port.recordingSettings('graph-1', 'take-1')).toEqual(settings);
    expect(await port.recordingSettings('graph-1', 'take-2')).toMatchObject({ folder: '/Desktop' });
    expect(await port.recorderStatus('graph-1', 'take-1')).toMatchObject({
      state: 'stopped',
      path: null,
      frames: 0,
    });
    await expect(port.controlRecorder('graph-1', 'take-1', { action: 'record' })).rejects.toThrow(
      'Recording requires the local audio engine.',
    );
  });

  it('returns default and supplied inventory without inventing plugins or devices', async () => {
    const { port, state } = fixture();
    expect(await port.listCapturableApplications()).toEqual([]);
    expect(await port.listAudioDevices()).toEqual([]);
    expect(await port.getProcessors()).toEqual({ protocolVersion: 1, processors: [] });
    expect(await port.getMeters()).toEqual({ protocolVersion: 1, revision: 1, nodes: [] });
    state.applications = [];
    state.audioDevices = [];
    state.processors = { protocolVersion: 1, processors: [] };
    state.meters = { protocolVersion: 1, revision: 9, nodes: [] };
    expect(await port.listCapturableApplications()).toBe(state.applications);
    expect(await port.listAudioDevices()).toBe(state.audioDevices);
    expect(await port.rescanProcessors([])).toBe(state.processors);
    expect(await port.getMeters()).toBe(state.meters);
    expect(await port.getStatus()).toBe(state.status);
  });

  it('holds editor state while acknowledging that native editor images are unavailable', async () => {
    const { port } = fixture();
    expect(await port.getProcessorEditor('fx')).toMatchObject({
      nodeId: 'fx',
      state: 'closed',
      failure: null,
    });
    expect(await port.setProcessorEditor('fx', true)).toMatchObject({ state: 'open' });
    expect(await port.getProcessorEditor('fx')).toMatchObject({ state: 'open' });
    expect(await port.setProcessorEditor('fx', false)).toMatchObject({ state: 'closed' });
    await expect(port.getProcessorEditorFrame('fx')).rejects.toThrow(
      'Native editor images are unavailable in Storybook',
    );
    await expect(
      port.sendProcessorEditorInput('fx', {
        instanceId: 'fx-1',
        session: 1,
        kind: 'cancel',
        x: 0,
        y: 0,
        modifiers: 0,
      }),
    ).resolves.toBeUndefined();
  });
});

describe('Storybook node controls', () => {
  it('preserves unrelated node settings while changing one gain and power state', async () => {
    const { port } = fixture();
    const initial = await port.getNodeControls('graph-1');
    expect(initial.nodes.map((node) => [node.nodeId, node.gainDb])).toEqual([
      ['source', 0],
      ['fx', undefined],
      ['sink', -3],
    ]);
    await port.updateNodeControl('graph-1', 'source', {
      expectedRevision: 1,
      gainDb: -12,
      enabled: false,
    });
    const controls = await port.getNodeControls('graph-1');
    expect(controls.nodes[0]).toMatchObject({
      gainDb: -12,
      control: { enabled: false, muted: false },
    });
    await port.updateNodeControl('graph-1', 'source', { expectedRevision: 2, muted: true });
    expect((await port.getNodeControls('graph-1')).nodes[0]).toMatchObject({
      gainDb: -12,
      control: { enabled: false, muted: true },
    });
    expect((await port.getGraph()).nodes[2]).toMatchObject({ config: { gainDb: -3 } });
    await expect(port.getNodeControls('missing')).rejects.toThrow('graph_invalid');
  });

  it.each([
    ['wrong graph', 'other', 'source', 1, 0, 'graph_revision_stale'],
    ['stale revision', 'graph-1', 'source', 0, 0, 'graph_revision_stale'],
    ['missing node', 'graph-1', 'absent', 1, 0, 'graph_revision_stale'],
    ['non-gain node', 'graph-1', 'fx', 1, 0, 'graph_invalid'],
    ['nonfinite gain', 'graph-1', 'source', 1, Number.NaN, 'graph_invalid'],
    ['gain below range', 'graph-1', 'source', 1, -61, 'graph_invalid'],
    ['gain above range', 'graph-1', 'source', 1, 13, 'graph_invalid'],
  ])(
    'rejects %s without mutating the preview graph',
    async (_name, graphId, nodeId, expectedRevision, gainDb, error) => {
      const { port } = fixture();
      const before = structuredClone(await port.getGraph());
      await expect(
        port.updateNodeControl(graphId, nodeId, { expectedRevision, gainDb }),
      ).rejects.toThrow(error);
      expect(await port.getGraph()).toEqual(before);
    },
  );
});

describe('Storybook graph catalog and presentation persistence', () => {
  it('saves graph-scoped layout independently and rejects stale layout writes', async () => {
    const { port } = fixture();
    await expect(port.getGraphLayout('missing')).rejects.toThrow('graph_invalid');
    expect(await port.getGraphLayout('graph-1')).toMatchObject({
      layoutRevision: 0,
      layout: { positions: {} },
    });
    const request = {
      expectedLayoutRevision: 0,
      layout: { positions: { source: { x: 12, y: 24 } } },
    };
    const saved = await port.setGraphLayout('graph-1', request);
    saved.layout.positions = {};
    expect(await port.getGraphLayout('graph-1')).toMatchObject({
      layoutRevision: 1,
      layout: request.layout,
    });
    await expect(port.setGraphLayout('graph-1', request)).rejects.toThrow('graph_revision_stale');
    expect((await port.getGraph()).revision).toBe(1);
  });

  it('restores controls from an inactive graph and replaces the final deleted graph with an empty one', async () => {
    const { port } = fixture();
    const created = await catalogMutation(port, 'create');
    expect(created.graphs).toHaveLength(2);
    expect((await port.getNodeControls('graph-1')).nodes).toHaveLength(3);
    await catalogMutation(port, 'rename', 'graph-1');
    await catalogMutation(port, 'delete', 'graph-1');
    expect((await port.getGraphCatalog()).graphs).toHaveLength(1);
    const id = (await port.getGraphCatalog()).activeId;
    const replaced = await catalogMutation(port, 'delete', id);
    expect(replaced.graphs).toHaveLength(1);
    expect(replaced.activeId).not.toBe(id);
    expect(replaced.document.nodes).toEqual([]);
  });

  it.each(['select', 'rename', 'delete'] as const)(
    'rejects %s of an unknown graph without invalidating catalog identity',
    async (action) => {
      const { port } = fixture();
      await expect(catalogMutation(port, action, 'missing')).rejects.toThrow('graph_invalid');
      expect((await port.getGraphCatalog()).activeId).toBe('graph-1');
    },
  );
});

function retry(): ProcessorRetryRequest {
  return {
    expectedRevision: 1,
    expectedCatalogRevision: 1,
    expectedInstanceId: 'instance-1',
    graphId: 'graph-1',
    nodeId: 'fx',
  };
}

describe('Storybook processor retry identity', () => {
  it('clears only the matching failed processor instance and advances the durable graph revision', async () => {
    const { state, port } = fixture();
    state.status.failedProcessors = [
      { graphId: 'graph-1', nodeId: 'fx', instanceId: 'instance-1' },
      { graphId: 'graph-2', nodeId: 'fx', instanceId: 'instance-1' },
      { graphId: 'graph-1', nodeId: 'other', instanceId: 'instance-1' },
      { graphId: 'graph-1', nodeId: 'fx', instanceId: 'instance-2' },
    ];
    expect(await port.retryProcessor(retry())).toMatchObject({ revision: 2, state: 'active' });
    expect((await port.getGraphCatalog()).document.revision).toBe(2);
    expect(state.status.failedProcessors).toHaveLength(3);
  });

  it.each([{ graphId: 'wrong' }, { expectedRevision: 0 }, { expectedCatalogRevision: 0 }])(
    'rejects stale retry fences %j before changing graph revision',
    async (overrides) => {
      const { port } = fixture();
      await expect(port.retryProcessor({ ...retry(), ...overrides })).rejects.toThrow(
        'graph_revision_stale',
      );
      expect((await port.getGraph()).revision).toBe(1);
    },
  );

  it.each([
    ['missing', 'node_not_found'],
    ['source', 'node_disabled'],
    ['fx', 'processor_instance_stale'],
  ])('rejects an ineligible retry for %s', async (nodeId, error) => {
    const { port } = fixture();
    await expect(port.retryProcessor({ ...retry(), nodeId })).rejects.toThrow(error);
  });

  it('rejects retry while the provider is disabled even if a failed instance is retained', async () => {
    const { state, port } = fixture();
    const provider = state.graph.nodes.find((node) => node.id === 'fx');
    if (!provider) throw new Error('Missing provider fixture');
    provider.control = { enabled: false, muted: false };
    await expect(port.retryProcessor(retry())).rejects.toThrow('node_disabled');
  });
});

describe('Storybook plugin parameter limits', () => {
  it.each([
    ['missing', 0, 0.5],
    ['source', 0, 0.5],
    ['fx', -1, 0.5],
    ['fx', 1.5, 0.5],
    ['fx', 1, Number.NaN],
    ['fx', 1, -0.1],
    ['fx', 1, 1.1],
  ])('rejects invalid parameter on %s (%s, %s)', async (nodeId, parameterId, value) => {
    const { port } = fixture();
    await expect(port.setProcessorParameter(nodeId, parameterId, value)).rejects.toThrow(
      'graph_invalid',
    );
    expect((await port.getGraph()).revision).toBe(1);
  });

  it('sets parameter values even when saved plugin state is empty', async () => {
    const { port } = fixture();
    await expect(port.setProcessorParameter('fx', 0, 0.5)).resolves.toMatchObject({
      nodeId: 'fx',
      parameterId: 0,
      value: 0.5,
    });
    expect((await port.getGraphCatalog()).document.nodes[1]?.config).toMatchObject({
      parameters: { parameterValues: { '0': 0.5 } },
    });
  });
});

describe('preview catalog concurrency and omitted control fields', () => {
  it('selects an existing saved graph and reads its committed runtime status', async () => {
    const { port } = fixture();
    await catalogMutation(port, 'create');
    const selected = await catalogMutation(port, 'select', 'graph-1');
    expect(selected.activeId).toBe('graph-1');
    expect(selected.document.nodes).toHaveLength(3);
    expect(await port.getGraphStatus()).toEqual({
      protocolVersion: 1,
      activeRevision: selected.document.revision,
      submittedRevision: selected.document.revision,
    });
  });

  it.each([
    { expectedRevision: 0, expectedCatalogRevision: 1 },
    { expectedRevision: 1, expectedCatalogRevision: 0 },
  ])('rejects stale catalog fences %j without replacing the current graph', async (fences) => {
    const { port } = fixture();
    await expect(
      port.mutateGraphCatalog({ action: 'create', name: 'Stale', ...fences }),
    ).rejects.toThrow('graph_revision_stale');
    expect((await port.getGraphCatalog()).graphs).toEqual([
      { id: 'graph-1', name: 'Untitled Graph' },
    ]);
  });

  it('defaults omitted power to enabled while accepting mute-only controls for providers', async () => {
    const { port } = fixture();
    await port.updateNodeControl('graph-1', 'fx', { expectedRevision: 1, muted: true });
    expect((await port.getNodeControls('graph-1')).nodes[1]).toMatchObject({
      control: { enabled: true, muted: true },
    });
    await port.updateNodeControl('graph-1', 'fx', { expectedRevision: 2 });
    expect((await port.getNodeControls('graph-1')).nodes[1]).toMatchObject({
      control: { enabled: true, muted: true },
    });
  });
});

it('returns isolated native activity fixtures and observed empty by default', async () => {
  const { state, port } = fixture();
  await expect(port.getVirtualDeviceAppActivity()).resolves.toEqual({});
  state.deviceAppActivity = {
    native: [
      { bundleId: 'app', displayName: 'App', processId: 1, playing: true, recording: false },
    ],
  };
  const snapshot = await port.getVirtualDeviceAppActivity();
  expect(snapshot).toEqual(state.deviceAppActivity);
  const cloned = snapshot.native?.[0];
  assert(cloned);
  cloned.playing = false;
  expect(state.deviceAppActivity.native?.[0]?.playing).toBe(true);
});

it('catalog deletion undo and redo preserve graph/node identities and reject absent history', async () => {
  const { port } = fixture();
  const mutate = async (action: import('../../domain/control/catalog').GraphCatalogAction) => {
    const catalog = await port.getGraphCatalog();
    return port.mutateGraphCatalog({
      ...action,
      expectedRevision: catalog.document.revision,
      expectedCatalogRevision: catalog.catalogRevision,
    });
  };
  const unchanged = await port.getGraphCatalog();
  await expect(mutate({ action: 'undo_delete' })).resolves.toEqual(unchanged);
  await expect(mutate({ action: 'redo_delete' })).resolves.toEqual(unchanged);
  const original = await port.getGraphCatalog();
  await mutate({ action: 'delete', id: original.activeId });
  const restored = await mutate({ action: 'undo_delete' });
  expect(restored.activeId).toBe(original.activeId);
  expect(restored.graphs).toEqual(original.graphs);
  expect(restored.document.nodes).toEqual(original.document.nodes);
  await mutate({ action: 'redo_delete' });
  expect((await port.getGraph()).nodes).toEqual([]);
  await mutate({ action: 'undo_delete' });
  expect((await port.getGraph()).nodes).toEqual(original.document.nodes);
});
