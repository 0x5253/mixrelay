import { describe, expect, it } from 'vitest';
import { fakeEngineFetch, fakeEngineInvoke } from '../../../test-support/fakeEngine';
import { ControlPortError } from '../../domain/control/errors';
import type { ControlPort } from '../../ports/control/ControlPort';
import { StorybookMockControlPort } from '../fake/StorybookMockControlPort';
import { HttpControlPort } from './HttpControlPort';
import { TauriControlPort } from './TauriControlPort';

function engine() {
  return new StorybookMockControlPort({
    graph: {
      schemaVersion: 2,
      revision: 1,
      edges: [],
      nodes: [
        { id: 'fx', type: 'provider', label: 'Effect', config: { typeId: 'vst3:fx' } },
        { id: 'take', type: 'recorder', label: 'Take', config: null },
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
    processors: {
      protocolVersion: 1,
      processors: [
        {
          displayName: 'Effect',
          format: 'vst3',
          hasEditor: true,
          layouts: [],
          defaultLayoutId: 'stereo',
          parameters: [
            {
              canAutomate: true,
              defaultValue: 0.5,
              id: 7,
              isReadOnly: false,
              name: 'Mix',
              stepCount: 0,
            },
          ],
          typeId: 'vst3:fx',
          vendor: 'Vendor',
          version: '1.0',
        },
      ],
    },
    applications: [],
    audioDevices: [],
    deviceAppActivity: {},
  });
}

/** Each implementation of the capability ports, backed by the same in-memory engine. */
const implementations: [string, (backend: StorybookMockControlPort) => ControlPort][] = [
  ['in-memory fake', (backend) => backend],
  ['HTTP adapter', (backend) => new HttpControlPort('/engine', fakeEngineFetch(backend))],
  [
    'Tauri adapter',
    (backend) =>
      new TauriControlPort(fakeEngineInvoke(backend), () => ({ onmessage: () => undefined })),
  ],
];

async function stale(promise: Promise<unknown>) {
  const error = await promise.catch((reason: unknown) => reason);
  expect(error).toBeInstanceOf(ControlPortError);
  return error instanceof ControlPortError ? [error.status, error.body.code] : undefined;
}

describe.each(implementations)('graph capability contract: %s', (_name, create) => {
  it('reads, applies at the expected revision and rejects a stale one', async () => {
    const port = create(engine());
    const graph = await port.getGraph();
    expect(graph.revision).toBe(1);
    await expect(port.applyGraph({ ...graph, revision: 2 })).resolves.toEqual({
      protocolVersion: 1,
      revision: 2,
      state: 'active',
    });
    expect((await port.getGraph()).revision).toBe(2);
    expect(await stale(port.applyGraph({ ...graph, revision: 2 }))).toEqual([
      409,
      'graph_revision_stale',
    ]);
    await expect(port.getGraphStatus()).resolves.toMatchObject({ activeRevision: 2 });
    await expect(port.getStatus()).resolves.toMatchObject({ driverState: 'ready' });
  });

  it('node controls apply at the graph revision', async () => {
    const port = create(engine());
    const controls = await port.getNodeControls?.('graph-1');
    expect(controls?.nodes.map((node) => node.nodeId)).toEqual(['fx', 'take']);
    await expect(
      port.updateNodeControl?.('graph-1', 'fx', { expectedRevision: 1, muted: true }),
    ).resolves.toMatchObject({ revision: 2, requiresGraphReload: false });
    expect(
      await stale(
        port.updateNodeControl?.('graph-1', 'fx', { expectedRevision: 1, muted: false }) ??
          Promise.resolve(),
      ),
    ).toEqual([409, 'graph_revision_stale']);
  });

  it('a retry against an old revision is rejected without retrying', async () => {
    const port = create(engine());
    const request = {
      graphId: 'graph-1',
      nodeId: 'fx',
      expectedRevision: 0,
      expectedCatalogRevision: 1,
      expectedInstanceId: 'instance',
    };
    expect(await stale(port.retryProcessor?.(request) ?? Promise.resolve())).toEqual([
      409,
      'graph_revision_stale',
    ]);
  });
});

describe.each(implementations)('telemetry and workspace contract: %s', (_name, create) => {
  it('telemetry: reads meters for the active revision', async () => {
    await expect(create(engine()).getMeters()).resolves.toEqual({
      nodes: [],
      protocolVersion: 1,
      revision: 1,
    });
  });

  it('workspace: catalog and layout writes are revision-fenced', async () => {
    const port = create(engine());
    const catalog = await port.getGraphCatalog?.();
    expect(catalog?.graphs).toEqual([{ id: 'graph-1', name: 'Untitled Graph' }]);
    const created = await port.mutateGraphCatalog?.({
      action: 'create',
      name: 'Second',
      expectedRevision: 1,
      expectedCatalogRevision: 1,
    });
    expect(created?.graphs.map((graph) => graph.name)).toEqual(['Untitled Graph', 'Second']);
    const layout = await port.getGraphLayout?.('graph-1');
    expect(layout).toEqual({ graphId: 'graph-1', layoutRevision: 0, layout: { positions: {} } });
    const request = { expectedLayoutRevision: 0, layout: { positions: { fx: { x: 1, y: 2 } } } };
    await expect(port.setGraphLayout?.('graph-1', request)).resolves.toMatchObject({
      layoutRevision: 1,
    });
    expect(await stale(port.setGraphLayout?.('graph-1', request) ?? Promise.resolve())).toEqual([
      409,
      'graph_revision_stale',
    ]);
  });
});

describe.each(implementations)('recorder, editor and inventory contract: %s', (_name, create) => {
  it('recorder: reports status and never fakes a take', async () => {
    const port = create(engine());
    await expect(port.recorderStatus('graph-1', 'take')).resolves.toMatchObject({
      state: 'stopped',
      frames: 0,
    });
    await expect(
      port.controlRecorder('graph-1', 'take', { action: 'record' }, 1),
    ).rejects.toThrow();
  });

  it('processor editor: opens, reads, sends input and sets parameters', async () => {
    const port = create(engine());
    await expect(port.getProcessorEditor('fx')).resolves.toMatchObject({ state: 'closed' });
    await expect(port.setProcessorEditor('fx', true, 1)).resolves.toMatchObject({ state: 'open' });
    await expect(port.getProcessorEditor('fx')).resolves.toMatchObject({ state: 'open' });
    await expect(port.getProcessorEditorFrame('fx')).rejects.toThrow();
    const input = { instanceId: 'i', session: 1, kind: 'down' as const, x: 1, y: 2, modifiers: 0 };
    await expect(port.sendProcessorEditorInput('fx', input)).resolves.toBeFalsy();
    await expect(port.setProcessorParameter('fx', 7, 0.25, 1)).resolves.toMatchObject({
      nodeId: 'fx',
      parameterId: 7,
      value: 0.25,
    });
  });

  it('inventory: reads and rescans processors', async () => {
    const port = create(engine());
    const catalog = await port.getProcessors();
    expect(catalog.processors.map((processor) => processor.typeId)).toEqual(['vst3:fx']);
    await expect(port.rescanProcessors(['/Plug-Ins'])).resolves.toEqual(catalog);
  });
});

describe.each(implementations.filter(([name]) => name !== 'HTTP adapter'))(
  'desktop capability contract: %s',
  (_name, create) => {
    it('recorder preferences round-trip per recorder node', async () => {
      const port = create(engine());
      const settings = { folder: '/Takes', sampleRate: 48000, encoding: 'float32' as const };
      await expect(port.setRecordingSettings('graph-1', 'take', settings, 1)).resolves.toEqual(
        settings,
      );
      await expect(port.recordingSettings('graph-1', 'take')).resolves.toEqual(settings);
    });

    it('inventory lists applications, devices and device clients', async () => {
      const port = create(engine());
      await expect(port.listCapturableApplications?.()).resolves.toEqual([]);
      await expect(port.listAudioDevices?.()).resolves.toEqual([]);
      await expect(port.getVirtualDeviceAppActivity?.()).resolves.toEqual({});
    });
  },
);

it('the fake engine rejects routes and commands it does not serve', async () => {
  const backend = engine();
  const response = await fakeEngineFetch(backend)('/engine/api/v1/unknown');
  expect(response.status).toBe(404);
  await expect(fakeEngineInvoke(backend)('unknown_command')).rejects.toThrow('unknown_command');
});
