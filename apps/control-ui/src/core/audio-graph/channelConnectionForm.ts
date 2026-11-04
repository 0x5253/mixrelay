import {
  type ChannelConnectionPlan,
  type ChannelConnectionRequest,
  planChannelConnections,
} from '../domain/graph/channelConnections';
import { type GraphConnection, graphConnectionRejection } from '../domain/graph/topology';
import { type AudioGraphModel, type AudioGraphNode, audioGraphNodeTitle } from './model';
import { namedAudioPorts } from './portNames';

export type NamedPort = ReturnType<typeof namedAudioPorts>[number];

/** Consecutive source outputs, from one output for `count` channels. */
export interface ChannelRange {
  startPortId: string;
  count: number;
}

/** What the user has chosen so far; everything else is derived from the graph. */
export interface ChannelConnectionChoices {
  /** `null` keeps the canvas selection until a range is chosen explicitly. */
  range: ChannelRange | null;
  targetNodeId: string | null;
  /** `null` until the user picks an input; the first input is assumed until then. */
  targetStartPortId: string | null;
}

export interface ChannelConnectionForm {
  /** The source's title, or its id once it has left the graph. */
  sourceTitle: string;
  outputs: readonly NamedPort[];
  /** The canvas-selected outputs that still exist. */
  selection: readonly NamedPort[];
  /** The range the source fields show, or `null` while a canvas selection is kept. */
  sourceRange: ChannelRange | null;
  targets: { value: string; label: string }[];
  target: AudioGraphNode | undefined;
  inputs: readonly NamedPort[];
  /** `null` asks the user to choose a channel. */
  targetAnchorPortId: string | null;
  request: ChannelConnectionRequest | null;
  plan: ChannelConnectionPlan;
}

const emptyPlan: ChannelConnectionPlan = { accepted: [], skipped: [] };

/** A canvas selection is kept; without one the form starts from every output. */
export function initialChoices(
  model: AudioGraphModel,
  sourceNodeId: string,
  sourcePortIds: readonly string[] | undefined,
): ChannelConnectionChoices {
  const outputs = sourceOutputs(model, sourceNodeId);
  const selected = outputs.some(({ port }) => sourcePortIds?.includes(port.id));
  return {
    range: selected ? null : { startPortId: outputs[0]?.port.id ?? '', count: outputs.length },
    targetNodeId: null,
    targetStartPortId: null,
  };
}

/** Everything the dialog shows and submits, derived from the graph as it is now. */
export function channelConnectionForm(
  model: AudioGraphModel,
  sourceNodeId: string,
  sourcePortIds: readonly string[] | undefined,
  choices: ChannelConnectionChoices,
): ChannelConnectionForm {
  const source = model.nodes.find((node) => node.id === sourceNodeId);
  const outputs = source ? namedAudioPorts(source.outputPorts) : [];
  const selection = outputs.filter(({ port }) => sourcePortIds?.includes(port.id));
  const target = model.nodes.find((node) => node.id === choices.targetNodeId);
  const inputs = target ? namedAudioPorts(target.inputPorts) : [];
  // A picked input that disappears must be chosen again rather than silently moving cables.
  const targetAnchorPortId =
    (choices.targetStartPortId === null
      ? inputs[0]?.port.id
      : inputs.find(({ port }) => port.id === choices.targetStartPortId)?.port.id) ?? null;
  // A canvas selection whose channels have all disappeared falls back to choosing a range.
  const sourceRange = choices.range ?? (selection.length ? null : { startPortId: '', count: 1 });
  const request = connectionRequest(
    sourceNodeId,
    sourceRange ? rangePorts(outputs, sourceRange) : selection,
    target?.id,
    targetAnchorPortId,
  );
  return {
    sourceTitle: source ? audioGraphNodeTitle(source) : sourceNodeId,
    outputs,
    selection,
    sourceRange,
    targets: targetOptions(model, sourceNodeId),
    target,
    inputs,
    targetAnchorPortId,
    request,
    plan: request ? planChannelConnections(model, request) : emptyPlan,
  };
}

/** The connections to commit: the graph can change while the form is open. */
export function acceptedConnections(
  model: AudioGraphModel,
  request: ChannelConnectionRequest | null,
): GraphConnection[] {
  return request ? planChannelConnections(model, request).accepted : [];
}

/** The smallest range covering a non-empty selection drawn from `outputs`. */
export function rangeFromSelection(
  outputs: readonly NamedPort[],
  selection: readonly NamedPort[],
): ChannelRange {
  const indexes = selection.map(({ port }) =>
    outputs.findIndex((output) => output.port.id === port.id),
  );
  const first = Math.min(...indexes);
  return {
    startPortId: (outputs[first] as NamedPort).port.id,
    count: Math.max(...indexes) - first + 1,
  };
}

/** A new first channel keeps the count where the remaining outputs allow it. */
export function rangeStartingAt(
  outputs: readonly NamedPort[],
  range: ChannelRange,
  startPortId: string,
): ChannelRange {
  const start = outputs.findIndex(({ port }) => port.id === startPortId);
  return { startPortId, count: Math.min(range.count, outputs.length - start) };
}

/** How many channels a range can still cover; 0 when its first channel disappeared. */
export function availableChannels(outputs: readonly NamedPort[], range: ChannelRange): number {
  const start = outputs.findIndex(({ port }) => port.id === range.startPortId);
  return start < 0 ? 0 : outputs.length - start;
}

/** The outputs a range covers; empty when its first channel no longer exists. */
function rangePorts(outputs: readonly NamedPort[], range: ChannelRange): NamedPort[] {
  const start = outputs.findIndex(({ port }) => port.id === range.startPortId);
  return start < 0 ? [] : outputs.slice(start, start + range.count);
}

/** Nodes with inputs a connection could reach; repeated titles get an ordinal so each
 * option is distinct. */
function targetOptions(
  model: AudioGraphModel,
  sourceNodeId: string,
): { value: string; label: string }[] {
  const nodes = model.nodes.filter((node) =>
    node.id === sourceNodeId ? acceptsSelfConnection(model, node) : node.inputPorts.length,
  );
  const titles = nodes.map(audioGraphNodeTitle);
  return nodes.map((node, index) => {
    const title = titles[index] as string;
    const repeats = titles.filter((other) => other === title).length;
    const ordinal = titles.slice(0, index).filter((other) => other === title).length + 1;
    return { value: node.id, label: repeats > 1 ? `${title} (${ordinal})` : title };
  });
}

function sourceOutputs(model: AudioGraphModel, sourceNodeId: string): NamedPort[] {
  const source = model.nodes.find((node) => node.id === sourceNodeId);
  return source ? namedAudioPorts(source.outputPorts) : [];
}

/** A node can feed itself only across a host boundary, e.g. a virtual device's
 * system-side output into its own system-side input. */
function acceptsSelfConnection(model: AudioGraphModel, node: AudioGraphNode) {
  return node.outputPorts.some((output) =>
    node.inputPorts.some(
      (input) =>
        graphConnectionRejection(model, {
          sourceNodeId: node.id,
          sourcePortId: output.id,
          targetNodeId: node.id,
          targetPortId: input.id,
        }) !== 'invalid',
    ),
  );
}

function connectionRequest(
  sourceNodeId: string,
  ports: readonly NamedPort[],
  targetNodeId: string | undefined,
  targetAnchorPortId: string | null,
): ChannelConnectionRequest | null {
  const anchor = ports[0];
  if (!anchor || !targetNodeId || !targetAnchorPortId) return null;
  return {
    sourceNodeId,
    sourcePortIds: ports.map(({ port }) => port.id),
    sourceAnchorPortId: anchor.port.id,
    targetNodeId,
    targetAnchorPortId,
  };
}
