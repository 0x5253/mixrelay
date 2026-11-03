import { removableSubscription } from '../../application/subscription';
import {
  type MidiBinding,
  type MidiControl,
  type MidiDevice,
  type MidiStatus,
  midiActions,
  midiModes,
} from '../../domain/midi';
import type { MidiCommand, MidiPort } from '../../ports/MidiPort';
import {
  array,
  boolean,
  type Decoder,
  InvalidResponseError,
  literal,
  nonNegativeInteger,
  nullable,
  number,
  object,
  string,
  tagged,
} from '../decode';
import { type NativeInvoke, type NativeListen, tauriInvoke, tauriListen } from './nativeInvoke';

function atMost(max: number): Decoder<number> {
  return (value, path) => {
    const decoded = nonNegativeInteger(value, path);
    if (decoded > max) throw new InvalidResponseError(path, `an integer up to ${max}`);
    return decoded;
  };
}

/** The gain range the desktop clamps MIDI bindings to. */
const gain: Decoder<number> = (value, path) => {
  const db = number(value, path);
  if (db < -60 || db > 12) throw new InvalidResponseError(path, 'a gain from -60 to 12 dB');
  return db;
};

const device = object<MidiDevice>({ id: string, name: string });

const control = object<MidiControl>({
  device,
  channel: atMost(15),
  kind: literal('note', 'cc'),
  number: atMost(127),
});

const binding = object<MidiBinding>({
  control,
  target: object({ graphId: string, nodeId: string }),
  action: literal(...midiActions),
  mode: literal(...midiModes),
  stepDb: literal(0.5, 1, 2, 3),
  graphName: string,
  nodeName: string,
  minDb: gain,
  maxDb: gain,
});

const learn = tagged<MidiStatus['learn']>('state', {
  idle: object({ state: literal('idle') }),
  waiting: object({ state: literal('waiting') }),
  captured: object({ state: literal('captured'), control }),
});

interface NativeMidiStatus {
  mapMode: boolean;
  bindings: MidiBinding[];
  devices: MidiDevice[];
  unavailable: string | null;
  learn: MidiStatus['learn'];
  /** The open graphs whose nodes can be mapped; null while no graph is available. */
  snapshot: {
    graphs: { id: string; name: string; nodes: { nodeId: string; label: string | null }[] }[];
  } | null;
}

const nativeMidiStatus = object<NativeMidiStatus>({
  mapMode: boolean,
  bindings: array(binding),
  devices: array(device),
  unavailable: nullable(string),
  learn,
  snapshot: nullable(
    object({
      graphs: array(
        object({
          id: string,
          name: string,
          nodes: array(object({ nodeId: string, label: nullable(string) })),
        }),
      ),
    }),
  ),
});

export function midiStatus(value: unknown): MidiStatus {
  const { snapshot, ...status } = nativeMidiStatus(value, 'midi');
  return {
    mapMode: status.mapMode,
    targetsAvailable: snapshot !== null,
    bindings: status.bindings,
    devices: status.devices,
    unavailable: status.unavailable,
    learn: status.learn,
    targets: (snapshot?.graphs ?? []).flatMap((graph) =>
      graph.nodes.map((node) => ({
        target: { graphId: graph.id, nodeId: node.nodeId },
        label: `${graph.name} / ${node.label ?? ''}`,
      })),
    ),
  };
}
export class TauriMidiPort implements MidiPort {
  constructor(
    private readonly invokeCommand: NativeInvoke = tauriInvoke,
    private readonly listen: NativeListen = tauriListen,
  ) {}
  subscribeMapToggle(listener: () => void) {
    let active = true;
    const remove = removableSubscription(
      this.listen('midi-map-toggle', () => {
        if (active) listener();
      }),
      (error) => console.error('MIDI menu listener failed', error),
    );
    return () => {
      active = false;
      remove();
    };
  }
  async command(name: MidiCommand, args?: Record<string, unknown>): Promise<MidiStatus> {
    return midiStatus(await this.invokeCommand(name, args));
  }
}
