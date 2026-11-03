import { expect, it } from 'vitest';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { i18n } from '../../core/i18n';
import { presentProcessorFailures } from './processorFailures';

it('marks only the failed provider in the selected graph, preserves audio, and clears presentation on recovery', async () => {
  await i18n.changeLanguage('en');
  const model: AudioGraphModel = {
    revision: 7,
    edges: [],
    nodes: ['voice', 'music', 'trim'].map((id) => ({
      id,
      position: { x: 0, y: 0 },
      inputPorts: [],
      outputPorts: [],
      appearance: { title: id, category: 'Processor', icon: 'FX' },
      runtime: { type: id === 'trim' ? 'gain' : 'provider', config: {} },
    })),
  };
  const presented = presentProcessorFailures(
    model,
    [
      { graphId: 'graph-1', nodeId: 'voice', instanceId: '1' },
      { graphId: 'graph-2', nodeId: 'music', instanceId: '2' },
      { graphId: 'graph-1', nodeId: 'trim', instanceId: '3' },
    ],
    'graph-1',
    i18n.t,
  );
  expect(presented.nodes[0]?.appearance).toMatchObject({
    status: 'Plug-in processing stopped',
    statusTone: 'critical',
  });
  expect(presented.nodes[0]?.runtime).toBe(model.nodes[0]?.runtime);
  expect(presented.revision).toBe(7);
  expect(presented.edges).toBe(model.edges);
  expect(presented.nodes[1]).toBe(model.nodes[1]);
  expect(presented.nodes[2]).toBe(model.nodes[2]);
  expect(model.nodes[0]?.appearance.status).toBeUndefined();
  expect(
    presentProcessorFailures(presented, undefined, 'graph-1', i18n.t).nodes[0]?.appearance.status,
  ).toBeUndefined();
});
