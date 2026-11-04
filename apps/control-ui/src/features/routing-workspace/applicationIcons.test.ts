import { expect, it } from 'vitest';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphDocument, RuntimeGraphNode } from '../../core/domain/control/graph';
import { presentRoutingApplicationIcons } from './applicationIcons';
import { runtimeGraphToAudioGraph } from './runtimeGraphProjection';

const empty: RuntimeGraphDocument = { schemaVersion: 2, revision: 1, nodes: [], edges: [] };
function documentWith(...nodes: RuntimeGraphNode[]): RuntimeGraphDocument {
  return { ...empty, nodes };
}
function node(model: AudioGraphModel): AudioGraphNode {
  const first = model.nodes[0];
  if (!first) throw new Error('Expected a materialized fixture node');
  return first;
}

it('does not infer an application icon after the captured application needs reselection', () => {
  const original = documentWith({
    id: 'app',
    type: 'application_capture',
    label: 'App',
    config: {
      requiresReselection: true,
      selector: { match: 'bundle_id', bundleId: 'example.app' },
      channels: 2,
    },
  });
  const model = runtimeGraphToAudioGraph(original);
  const marked = {
    ...model,
    nodes: model.nodes.map((item) => ({
      ...item,
      appearance: { ...item.appearance, iconUrl: 'stale' },
    })),
  };
  expect(node(presentRoutingApplicationIcons(marked, [])).appearance.iconUrl).toBeUndefined();
});
