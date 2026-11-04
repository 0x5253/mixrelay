import type { AudioGraphNode } from '../../core/audio-graph/model';
import type { RuntimeGraphNode } from '../../core/domain/control/graph';
import type { EndpointUpdate } from '../../core/ui/AudioNodeEndpointOptions';
import { directNumber, directString, nestedNumber, objectValue } from './domain/configValues';
import type { GraphPresentation } from './graphPresentation';
import { channelPorts, type RuntimeAudioNode } from './runtimeAudioNode';

const FROM_OS_PORT_ID = 'from_os';
export const TO_OS_PORT_ID = 'to_os';
const SOURCE_NODE_SUFFIX = '::host-source';
const SINK_NODE_SUFFIX = '::host-sink';

export interface VirtualEndpointUpdate {
  sampleFormat?: EndpointUpdate['sampleFormat'];
  /** Dedicated native-device transaction, never serialized into the graph. */
  sampleRate?: number;
  captureChannels: number;
  displayName: string;
  renderChannels: number;
}

export function virtualEndpointIdentity(node: AudioGraphNode): string | null {
  return node.runtime?.type === 'virtual_endpoint' ? directString(node.runtime.config, 'id') : null;
}

export function virtualEndpointNodes(
  node: RuntimeGraphNode,
  presentation: GraphPresentation,
  reservedIds: ReadonlySet<string>,
): RuntimeAudioNode[] {
  const title = directString(node.config, 'displayName');
  const renderChannels = nestedNumber(node.config, 'render', 'channels') ?? 0;
  const captureChannels = nestedNumber(node.config, 'capture', 'channels') ?? 0;
  const reference = objectValue(objectValue(node.config)?.reference);
  if (!title || !reference) return [];
  const control = endpointControl(title, renderChannels, captureChannels, presentation);
  const runtime = { config: node.config, type: node.type };
  const nodes: RuntimeAudioNode[] = [];
  if (reference.render === true && captureChannels > 0) {
    nodes.push({
      appearance: {
        category: presentation.virtualInput,
        gainControl: {
          label: presentation.sourceGain,
          valueDb: directNumber(node.config, 'gainDb') ?? 0,
        },
        channelCount: captureChannels,
        endpointControl: control,
        icon: 'IN',
        meterBanks: [{ channels: captureChannels, nodeId: node.id, portId: FROM_OS_PORT_ID }],
        optionsLabel: presentation.options,
        title,
      },
      id: virtualViewId(node.id, 'source', reservedIds),
      inputPorts: [],
      outputPorts: channelPorts(
        FROM_OS_PORT_ID,
        captureChannels,
        presentation.virtualSourcePort,
        'source',
      ),
      position: { x: 48, y: 96 },
      runtime: { ...runtime, binding: { nodeId: node.id, side: 'source' } },
    });
  }
  if (reference.capture === true && renderChannels > 0) {
    nodes.push({
      appearance: {
        category: presentation.virtualOutput,
        channelCount: renderChannels,
        endpointControl: control,
        icon: 'OUT',
        meterBanks: [{ channels: renderChannels, nodeId: node.id, portId: TO_OS_PORT_ID }],
        optionsLabel: presentation.options,
        title,
      },
      id: virtualViewId(node.id, 'sink', reservedIds),
      inputPorts: channelPorts(TO_OS_PORT_ID, renderChannels, presentation.virtualSinkPort, 'sink'),
      outputPorts: [],
      position: { x: 728, y: 96 },
      runtime: { ...runtime, binding: { nodeId: node.id, side: 'sink' } },
    });
  }
  return nodes;
}

function endpointControl(
  displayName: string,
  renderChannels: number,
  captureChannels: number,
  presentation: GraphPresentation,
): NonNullable<AudioGraphNode['appearance']['endpointControl']> {
  return {
    captureChannels,
    channelsLabel: presentation.channels,
    displayName,
    nameLabel: presentation.deviceName,
    renderChannels,
  };
}

function virtualViewId(
  nodeId: string,
  side: 'source' | 'sink',
  reservedIds: ReadonlySet<string>,
): string {
  let id = `${nodeId}${side === 'source' ? SOURCE_NODE_SUFFIX : SINK_NODE_SUFFIX}`;
  if (!reservedIds.has(id)) return id;
  // Escaped IDs end in `]`, unlike ordinary virtual view IDs. JSON keeps each
  // node/side pair distinct; leading # characters only avoid actual node IDs.
  id = `@virtual:${JSON.stringify([nodeId, side])}`;
  while (reservedIds.has(id)) id = `#${id}`;
  return id;
}
