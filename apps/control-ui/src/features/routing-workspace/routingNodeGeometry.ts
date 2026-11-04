import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type {
  ExternalProcessorDescriptor,
  ProviderNodeConfig,
  ProviderPorts,
} from '../../core/domain/control/processors';
import { directNumber } from './domain/configValues';
import type { GraphPresentation } from './graphPresentation';
import { externalProcessorNode } from './processorNodes';
import { AUDIO_PORT_ID, channelPorts, runtimeNodeId } from './runtimeAudioNode';

/** Channel edits are graph changes; an active take is guarded by the recorder control boundary. */
export function updateRecorderChannels(
  model: AudioGraphModel,
  nodeId: string,
  channels: number,
): AudioGraphModel {
  if (!Number.isInteger(channels) || channels < 1 || channels > 8) return model;
  const selected = model.nodes.find((node) => node.id === nodeId);
  if (
    selected?.runtime?.type !== 'recorder' ||
    directNumber(selected.runtime.config, 'channels') === channels
  )
    return model;
  const nodes = model.nodes.map((node) =>
    node === selected
      ? {
          ...node,
          appearance: {
            ...node.appearance,
            channelCount: channels,
            meterBanks: [{ channels, nodeId: runtimeNodeId(node), portId: AUDIO_PORT_ID }],
          },
          inputPorts: channelPorts(AUDIO_PORT_ID, channels, node.appearance.title),
          runtime: { ...selected.runtime, type: 'recorder', config: { channels } },
        }
      : node,
  );
  return updateNodeGeometry(model, nodes);
}

/** Switches a plug-in node to another catalog layout. Channel ports keep their IDs, so
 * cables on channels that still exist stay; cables to removed channels are dropped. */
export function changeProcessorLayout(
  model: AudioGraphModel,
  nodeId: string,
  processor: ExternalProcessorDescriptor,
  ports: ProviderPorts,
  presentation: GraphPresentation,
): AudioGraphModel {
  const selected = model.nodes.find((node) => node.id === nodeId);
  const config = selected?.runtime?.config as ProviderNodeConfig | undefined;
  if (
    selected?.runtime?.type !== 'provider' ||
    config?.typeId !== processor.typeId ||
    config.ports?.layoutId === ports.layoutId
  )
    return model;
  const rebuilt = externalProcessorNode(
    { id: runtimeNodeId(selected), position: selected.position },
    processor,
    ports,
    presentation,
  );
  const nodes = model.nodes.map((node) =>
    node === selected
      ? {
          ...node,
          appearance: {
            ...node.appearance,
            channelCount: rebuilt.appearance.channelCount,
            meterBanks: rebuilt.appearance.meterBanks,
          },
          inputPorts: rebuilt.inputPorts,
          outputPorts: rebuilt.outputPorts,
          runtime: { ...selected.runtime, type: 'provider', config: { ...config, ports } },
        }
      : node,
  );
  return updateNodeGeometry(model, nodes);
}

function updateNodeGeometry(model: AudioGraphModel, nodes: AudioGraphNode[]): AudioGraphModel {
  const validNodePorts = new Map(
    nodes.map((node) => [
      node.id,
      {
        inputs: new Set(node.inputPorts.map((port) => port.id)),
        outputs: new Set(node.outputPorts.map((port) => port.id)),
      },
    ]),
  );
  const edges = model.edges.filter((edge) => {
    const source = validNodePorts.get(edge.sourceNodeId);
    const target = validNodePorts.get(edge.targetNodeId);
    return source?.outputs.has(edge.sourcePortId) && target?.inputs.has(edge.targetPortId);
  });
  return { ...model, edges, nodes, revision: model.revision + 1 };
}
