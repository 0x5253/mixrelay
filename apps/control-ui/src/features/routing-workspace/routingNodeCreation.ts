import {
  type AudioGraphModel,
  type AudioGraphNode,
  type AudioGraphPort,
  type AudioGraphPosition,
  graphNodeDefaultWidth,
} from '../../core/audio-graph/model';
import type { RuntimeGraphNode } from '../../core/domain/control/graph';
import type {
  VirtualEndpointSettings,
  VirtualEndpointSpec,
} from '../../core/domain/control/status';
import { addGraphNode, connectGraph } from '../../core/domain/graph/topology';
import { nodeInsertionGap } from '../../core/theme/tokens';
import { type ChainKind, chainEffects, effectConfig } from './builtinEffects';
import { directNumber, directString, objectValue } from './domain/configValues';
import { defaultPresentation, type GraphPresentation } from './graphPresentation';
import { applicationNode, audioInputNode, audioOutputNode, recorderNode } from './ioNodes';
import type { RoutingNodeRequest } from './nodeRequests';
import {
  builtinEffectNode,
  compressorNode,
  externalProcessorNode,
  limiterNode,
} from './processorNodes';
import { type NodeBase, type RuntimeAudioNode, runtimeNodeId } from './runtimeAudioNode';
import { virtualEndpointNodes } from './virtualEndpointNodes';

type MaterializedRoutingNodeRequest = Exclude<
  RoutingNodeRequest,
  { kind: 'virtual_input' } | { kind: 'virtual_output' } | { kind: 'effect_chain' }
>;

export function addRoutingNode(
  model: AudioGraphModel,
  request: RoutingNodeRequest,
  position: AudioGraphPosition,
  presentation: GraphPresentation = defaultPresentation,
  endpointSettings?: VirtualEndpointSettings,
): AudioGraphModel {
  if (request.kind === 'effect_chain')
    return addEffectChain(model, request.preset, position, presentation);
  if (request.kind === 'virtual_input' || request.kind === 'virtual_output') {
    if (
      endpointSettings &&
      !endpointSettings.canCreate &&
      !endpointSettings.devices.some((device) => device.spec.id === request.spec.id)
    )
      return model;
    return addVirtualEndpointSide(model, request, position, presentation);
  }
  const id = newNodeId(request.kind);
  const base = {
    id,
    position,
  };
  const node = createRoutingNode(base, request, presentation);
  return addGraphNode(model, node);
}

function addEffectChain(
  model: AudioGraphModel,
  preset: ChainKind,
  position: AudioGraphPosition,
  presentation: GraphPresentation,
): AudioGraphModel {
  let next = model;
  let previous: AudioGraphNode | undefined;
  let nextX = position.x;
  const requests: Extract<RoutingNodeRequest, { kind: 'effect' | 'compressor' }>[] = chainEffects(
    preset,
  ).map((effect) => ({
    kind: 'effect',
    effect: effect.kind,
    parameters: effect.parameters,
  }));
  if (preset === 'radio') requests.push({ kind: 'compressor' });
  for (const item of requests) {
    const node = createRoutingNode(
      { id: newNodeId(item.kind), position: { x: nextX, y: position.y } },
      item,
      presentation,
    );
    next = addGraphNode(next, node);
    if (previous) {
      // Generated processor port arrays are dense; connect only channels present on both nodes.
      for (
        let channel = 0;
        channel < Math.min(previous.outputPorts.length, node.inputPorts.length);
        channel += 1
      ) {
        const source = previous.outputPorts[channel] as AudioGraphPort;
        const target = node.inputPorts[channel] as AudioGraphPort;
        next = connectGraph(next, {
          sourceNodeId: previous.id,
          sourcePortId: source.id,
          targetNodeId: node.id,
          targetPortId: target.id,
        });
      }
    }
    previous = node;
    nextX += graphNodeDefaultWidth(node) + nodeInsertionGap;
  }
  return { ...next, revision: model.revision + 1 };
}

function addVirtualEndpointSide(
  model: AudioGraphModel,
  request: Extract<RoutingNodeRequest, { spec: VirtualEndpointSpec }>,
  position: AudioGraphPosition,
  presentation: GraphPresentation,
): AudioGraphModel {
  const { kind, spec } = request;
  const sibling = model.nodes.find(
    (node) =>
      node.runtime?.type === 'virtual_endpoint' &&
      directString(node.runtime.config, 'id') === spec.id,
  );
  const nodeId = sibling ? runtimeNodeId(sibling) : `virtual-${crypto.randomUUID()}`;
  const sideName = kind === 'virtual_input' ? 'source' : 'sink';
  if (
    model.nodes.some(
      (node) => node.runtime?.binding?.nodeId === nodeId && node.runtime.binding.side === sideName,
    )
  )
    return model;
  const siblingReference = objectValue(objectValue(sibling?.runtime?.config)?.reference);
  const config = {
    ...spec,
    gainDb: directNumber(sibling?.runtime?.config, 'gainDb') ?? 0,
    reference: {
      render: kind === 'virtual_input' || siblingReference?.render === true,
      capture: kind === 'virtual_output' || siblingReference?.capture === true,
    },
  };
  const endpoint: RuntimeGraphNode = {
    config,
    id: nodeId,
    label: spec.displayName,
    type: 'virtual_endpoint',
  };
  const side = virtualEndpointNodes(
    endpoint,
    presentation,
    new Set(model.nodes.map(runtimeNodeId)),
  ).find((node) => node.runtime?.binding?.side === sideName);
  return side ? addGraphNode(model, { ...side, position }) : model;
}

function createRoutingNode(
  base: NodeBase,
  request: MaterializedRoutingNodeRequest,
  presentation: GraphPresentation,
): RuntimeAudioNode {
  if (request.kind === 'application') return applicationNode(base, request, presentation);
  if (request.kind === 'audio_input') return audioInputNode(base, request, presentation);
  if (request.kind === 'audio_output') return audioOutputNode(base, request, presentation);
  if (request.kind === 'recorder') return recorderNode(base, 2, presentation);
  if (request.kind === 'compressor') return compressorNode(base, presentation);
  if (request.kind === 'limiter') return limiterNode(base, presentation);
  if (request.kind === 'effect') {
    const config = effectConfig(request.effect, request.parameters);
    const effect = {
      kind: request.effect,
      parameters: config.effect.parameters,
      channels: config.channels,
    };
    return builtinEffectNode(base, config, effect, presentation);
  }
  return externalProcessorNode(base, request.processor, request.ports, presentation);
}

function newNodeId(kind: RoutingNodeRequest['kind']): string {
  return `${kind}-${crypto.randomUUID()}`;
}
