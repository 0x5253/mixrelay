import type { NodeControlState, RuntimeGraphEdge } from '../domain/control/graph';
import type { GraphEdge, GraphNode, GraphPort, GraphTopology } from '../domain/graph/topology';
import { nodeWidths } from '../theme/tokens';
import type { NodeChoiceControl } from '../ui/AudioNodeCard';
import type { EndpointControl } from '../ui/AudioNodeEndpointOptions';
import type { AudioNodeParameterControl } from '../ui/AudioNodeParameterGrid';
import type { DeviceAppActivityProps } from '../ui/DeviceAppActivity';
import type { DynamicsMeterContract } from '../ui/DynamicsMeter';
import type { GraphicEqualizerContract } from '../ui/GraphicEqualizer';
import type { StatusTone } from '../ui/StatusBadge';

/** Shared default geometry; explicit resized dimensions remain authoritative. */
export function graphNodeDefaultWidth(node: AudioGraphNode) {
  if (node.appearance.equalizer) return nodeWidths.equalizer;
  return node.appearance.dynamics ? nodeWidths.dynamics : nodeWidths.default;
}

/** Preserve complete processor faces while standard nodes retain their resize floor. */
export function graphNodeMinimumWidth(node: AudioGraphNode) {
  return node.appearance.equalizer || node.appearance.dynamics ? graphNodeDefaultWidth(node) : 180;
}

/** Apply the current resize floor without changing height or the audio document. */
export function normalizeAudioGraphNodeSize(
  node: AudioGraphNode,
  size: AudioGraphNodeSize | undefined,
) {
  if (!size) return undefined;
  const width = Math.max(graphNodeMinimumWidth(node), size.width);
  return width === size.width ? size : { ...size, width };
}

export interface AudioGraphPosition {
  x: number;
  y: number;
}

export interface AudioGraphNodeSize {
  width: number;
  height: number;
}

export interface AudioGraphPort extends GraphPort {
  channelIndex?: number;
  label: string;
  runtimePortId?: string;
}

export interface AudioGraphNodeAppearance {
  deviceAppActivity?: DeviceAppActivityProps;
  dynamics?: DynamicsMeterContract;
  equalizer?: GraphicEqualizerContract;
  processingTimeNs?: number;
  category: string;
  channelCount?: number;
  detail?: string;
  endpointControl?: EndpointControl;
  /** A plug-in's channel layout, chosen in the node's Options. */
  channelLayoutControl?: NodeChoiceControl;
  gainControl?: { label: string; valueDb: number };
  hasProcessorEditor?: boolean;
  icon: string;
  iconUrl?: string;
  meterBanks?: readonly AudioGraphMeterBank[];
  optionsLabel?: string;
  parameterControls?: readonly AudioNodeParameterControl[];
  status?: string;
  statusTone?: StatusTone;
  offline?: boolean;
  title: string;
}

/** Meter channels stay attached to the exact native node output port. */
export interface AudioGraphMeterBank {
  channels: number;
  label?: string;
  nodeId: string;
  portId: string;
}

/** A graph node with its presentation; topology rules see only its ID and ports. */
export interface AudioGraphNode extends GraphNode {
  /** UI instance alias; audio identity, runtime label and config remain unchanged. */
  displayName?: string;
  appearance: AudioGraphNodeAppearance;
  inputPorts: AudioGraphPort[];
  outputPorts: AudioGraphPort[];
  position: AudioGraphPosition;
  size?: AudioGraphNodeSize;
  runtime?: {
    control?: NodeControlState;
    /** Unresolved presentation retains the original nullable label, not its fallback title. */
    label?: string | null;
    binding?: { nodeId: string; side: 'source' | 'sink' };
    config: unknown;
    type: string;
  };
}

export function audioGraphNodeTitle(node: AudioGraphNode): string {
  return node.displayName ?? node.appearance.title;
}

/** The presented graph: topology plus appearance, geometry and native projections. */
export interface AudioGraphModel extends GraphTopology<AudioGraphNode> {
  /** Native topology plus its view projection; unedited groups keep their exact IDs/mapping. */
  edgeProjections?: { edge: RuntimeGraphEdge; views: GraphEdge[] }[];
  edges: GraphEdge[];
  nodes: AudioGraphNode[];
}

export function moveAudioGraphNode(
  model: AudioGraphModel,
  nodeId: string,
  position: AudioGraphPosition,
): AudioGraphModel {
  const nodeIndex = model.nodes.findIndex((node) => node.id === nodeId);
  const node = model.nodes[nodeIndex];
  if (!node || (node.position.x === position.x && node.position.y === position.y)) return model;
  return {
    ...model,
    nodes: model.nodes.map((node, index) => (index === nodeIndex ? { ...node, position } : node)),
  };
}

/** Presentation-only resize; it never advances the audio graph revision. */
export function resizeAudioGraphNode(
  model: AudioGraphModel,
  nodeId: string,
  size: AudioGraphNodeSize,
): AudioGraphModel {
  if (!Number.isFinite(size.width) || !Number.isFinite(size.height)) return model;
  const node = model.nodes.find((candidate) => candidate.id === nodeId);
  if (!node) return model;
  const bounded = {
    width: Math.max(graphNodeMinimumWidth(node), Math.min(4096, size.width)),
    height: Math.max(26, Math.min(4096, size.height)),
  };
  if (node.size?.width === bounded.width && node.size.height === bounded.height) return model;
  return {
    ...model,
    nodes: model.nodes.map((candidate) =>
      candidate === node ? { ...candidate, size: bounded } : candidate,
    ),
  };
}
