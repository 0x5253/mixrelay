import { describe, expect, it, vi } from 'vitest';
import type { GraphApplyRequest, RuntimeGraphDocument } from '../../domain/control/graph';
import { StorybookMockControlPort } from '../fake/StorybookMockControlPort';
import { HttpControlPort } from './HttpControlPort';
import { TauriControlPort } from './TauriControlPort';

const graph: RuntimeGraphDocument = {
  schemaVersion: 2,
  revision: 1,
  edges: [],
  nodes: [
    {
      id: 'p',
      type: 'provider',
      label: null,
      config: {
        typeId: 'vst3:test',
        parameters: {
          stateRef: { sha256: 'b'.repeat(64), byteLength: 9 },
          parameterValues: { '1': 0.7, '2': 0.8 },
        },
      },
    },
  ],
};
const intent = [{ nodeId: 'p', typeId: 'vst3:test', parameterId: 1, value: 0.3 }];
const request: GraphApplyRequest = { ...graph, revision: 2, processorParameterChanges: intent };

it('HTTP forwards explicit intent and state references without changing document fields', async () => {
  const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
    Response.json({
      protocolVersion: 1,
      revision: 2,
      state: 'active',
    }),
  );
  await new HttpControlPort('http://control', fetcher).applyGraph(request);
  expect(fetcher).toHaveBeenCalledWith('http://control/api/v1/graph', {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(request),
  });
});

it('Tauri preserves intent in the existing document argument', async () => {
  const invoke = vi
    .fn()
    .mockResolvedValue({ status: 200, body: { protocolVersion: 1, revision: 2, state: 'active' } });
  await new TauriControlPort(invoke).applyGraph(request);
  expect(invoke).toHaveBeenCalledWith('control_apply_graph', { document: request });
});

function fixture() {
  return new StorybookMockControlPort({
    graph: structuredClone(graph),
    graphStatus: { protocolVersion: 1, activeRevision: 1, submittedRevision: 1 },
    status: {
      protocolVersion: 1,
      driverState: 'ready',
      sampleRate: 48000,
      bufferFrames: 128,
      underruns: 0,
      overruns: 0,
    },
  });
}
const stale: RuntimeGraphDocument = {
  ...graph,
  revision: 2,
  nodes: [
    {
      id: 'p',
      type: 'provider',
      label: null,
      config: {
        typeId: 'vst3:test',
        parameters: {
          stateRef: { sha256: 'a'.repeat(64), byteLength: 8 },
          parameterValues: { '1': 0.1, '2': 0.2 },
        },
      },
    },
  ],
};

describe('storybook preserves the live-state intent contract without pretending to run a vendor', () => {
  it('preserves current provider state for raw structural requests', async () => {
    const port = fixture();
    await port.applyGraph(stale);
    expect((await port.getGraph()).nodes[0]?.config).toEqual(graph.nodes[0]?.config);
  });
  it('overlays only explicit IDs and does not persist request intent in graph/catalog data', async () => {
    const port = fixture();
    await port.applyGraph({ ...stale, processorParameterChanges: intent });
    expect((await port.getGraph()).nodes[0]?.config).toEqual({
      typeId: 'vst3:test',
      parameters: {
        stateRef: { sha256: 'b'.repeat(64), byteLength: 9 },
        parameterValues: { '1': 0.3, '2': 0.8 },
      },
    });
    expect(await port.getGraph()).not.toHaveProperty('processorParameterChanges');
    expect((await port.getGraphCatalog()).document).not.toHaveProperty('processorParameterChanges');
  });
  it('keeps acknowledged direct parameters across later structural graph replacement', async () => {
    const port = fixture();
    await port.setProcessorParameter('p', 2, 0.9);
    await port.applyGraph(stale);
    expect((await port.getGraph()).nodes[0]?.config).toMatchObject({
      parameters: { parameterValues: { '2': 0.9 } },
    });
  });
  it('rejects duplicate or incompatible explicit intent instead of overwriting an unrelated instance', async () => {
    const port = fixture();
    await expect(
      port.applyGraph({ ...stale, processorParameterChanges: [...intent, ...intent] }),
    ).rejects.toThrow('graph_invalid');
    await expect(
      port.applyGraph({
        ...stale,
        processorParameterChanges: [{ nodeId: 'p', typeId: 'other', parameterId: 1, value: 0.3 }],
      }),
    ).rejects.toThrow('graph_invalid');
    expect(await port.getGraph()).toEqual(graph);
  });
});
