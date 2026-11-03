import type { GraphEdge } from '../domain/graph/topology';
import { type AudioGraphNode, type AudioGraphPort, audioGraphNodeTitle } from './model';

/** Named cable endpoints; channel numbers match the port handles. A port name is
 * present only when another port on the same side shares that channel number. */
export interface AudioGraphEdgeEndpoints {
  source: string;
  sourceChannel: number;
  sourcePort?: string;
  target: string;
  targetChannel: number;
  targetPort?: string;
}

/** Accessible port names on one side with their handle channel numbers (an explicit
 * channel index, otherwise the port position). Ports that share both a label and a
 * channel number (e.g. two buses named Main) get an ordinal in port-ID order, skipping
 * names already used on that channel. Names survive reordering while each port's channel
 * stays the same, which explicit channelIndex values guarantee. */
export function namedAudioPorts(ports: readonly AudioGraphPort[]) {
  const entries = ports.map((port, index) => ({ port, channel: (port.channelIndex ?? index) + 1 }));
  const names = new Map(entries.map(({ port }) => [port.id, port.label]));
  const groups = new Map<number, AudioGraphPort[]>();
  for (const { port, channel } of entries) {
    const group = groups.get(channel);
    if (group) group.push(port);
    else groups.set(channel, [port]);
  }
  for (const group of groups.values()) {
    const counts = new Map<string, number>();
    for (const port of group) counts.set(port.label, (counts.get(port.label) ?? 0) + 1);
    const taken = new Set(
      group.filter((port) => counts.get(port.label) === 1).map((port) => port.label),
    );
    const repeated = group.filter((port) => (counts.get(port.label) as number) > 1);
    for (const port of repeated.sort((a, b) => (a.id < b.id ? -1 : 1))) {
      let ordinal = 1;
      while (taken.has(`${port.label} (${ordinal})`)) ordinal += 1;
      const name = `${port.label} (${ordinal})`;
      taken.add(name);
      names.set(port.id, name);
    }
  }
  return entries.map((entry) => ({ ...entry, name: names.get(entry.port.id) as string }));
}

type PortEndpoint = { channel: number; port: string | undefined };
/** One endpoint index per port list. Port lists are immutable and keep their identity across
 * reconciliations (structural sharing), so cable names are not recomputed per cable or update. */
const portEndpointIndex = new WeakMap<
  readonly AudioGraphPort[],
  ReadonlyMap<string, PortEndpoint>
>();

function portEndpoint(ports: readonly AudioGraphPort[], portId: string) {
  let index = portEndpointIndex.get(ports);
  if (!index) {
    const named = namedAudioPorts(ports);
    const perChannel = new Map<number, number>();
    for (const { channel } of named) perChannel.set(channel, (perChannel.get(channel) ?? 0) + 1);
    index = new Map(
      named.map(({ port, channel, name }) => [
        port.id,
        // A port name is only needed where several ports share the channel number.
        { channel, port: (perChannel.get(channel) as number) > 1 ? name : undefined },
      ]),
    );
    portEndpointIndex.set(ports, index);
  }
  return index.get(portId) ?? null;
}

export function audioGraphEdgeEndpoints(
  nodesById: ReadonlyMap<string, AudioGraphNode>,
  graphEdge: GraphEdge,
): AudioGraphEdgeEndpoints | null {
  const source = nodesById.get(graphEdge.sourceNodeId);
  const target = nodesById.get(graphEdge.targetNodeId);
  if (!source || !target) return null;
  const from = portEndpoint(source.outputPorts, graphEdge.sourcePortId);
  const to = portEndpoint(target.inputPorts, graphEdge.targetPortId);
  if (!from || !to) return null;
  return {
    source: audioGraphNodeTitle(source),
    sourceChannel: from.channel,
    sourcePort: from.port,
    target: audioGraphNodeTitle(target),
    targetChannel: to.channel,
    targetPort: to.port,
  };
}
