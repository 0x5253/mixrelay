import type { ControlStateSnapshot, ControlTelemetry } from '../../domain/control/state';
import {
  array,
  literal,
  nonNegativeInteger,
  nullable,
  object,
  optional,
  string,
  unknownValue,
} from '../decode';
import { controlStateSnapshot, meterTelemetry } from './protocolDecoders';

interface MeterBinding {
  nodeId: string;
  portId: string;
  channels: number;
  inputChannels: number | null;
}
interface PackedMeters {
  protocolVersion: 1;
  revision: number;
  /** Present when the bindings differ from the ones this stream carried last. */
  bindings?: MeterBinding[];
  samples: string;
}
interface PackedTelemetry {
  engineIncarnation: string;
  sequence: number;
  meters: PackedMeters;
  /** Decoded on its own, so a bad snapshot never stops the meters. */
  state?: unknown;
}

const packedTelemetry = object<PackedTelemetry>({
  engineIncarnation: string,
  sequence: nonNegativeInteger,
  meters: object<PackedMeters>({
    protocolVersion: literal(1),
    revision: nonNegativeInteger,
    bindings: optional(
      array(
        object<MeterBinding>({
          nodeId: string,
          portId: string,
          channels: nonNegativeInteger,
          inputChannels: nullable(nonNegativeInteger),
        }),
      ),
    ),
    samples: string,
  }),
  state: unknownValue,
});

/** Reports a rejected part of a stream; the stream itself may continue. */
export type TelemetryReport = (message: string, error: unknown) => void;

const consoleReport: TelemetryReport = (message, error) => console.error(message, error);

/** Decodes one ordered stream (a native channel or an SSE response). Meter bindings are
 * sent only when they change, so each stream needs its own decoder. */
export class TelemetryDecoder {
  private bindings?: MeterBinding[];
  private stateRejected = false;

  constructor(private readonly report: TelemetryReport = consoleReport) {}

  /** Whether this stream carried a snapshot that broke its contract. Such a window still
   * counts as the stream's opening snapshot. */
  get snapshotRejected(): boolean {
    return this.stateRejected;
  }

  /** A window the native channel already parsed, with unpacked meters. */
  window(message: unknown): ControlTelemetry {
    const window = meterTelemetry(message, 'telemetry window');
    const state = this.state(
      typeof message === 'object' && message !== null && 'state' in message
        ? message.state
        : undefined,
    );
    return { ...window, state };
  }

  /** The snapshot, or undefined when it breaks its contract: the window's meters still
   * count, and consumers keep waiting for the next valid snapshot. Reported once per stream. */
  private state(value: unknown): ControlStateSnapshot | null | undefined {
    if (value === undefined || value === null) return value;
    try {
      return controlStateSnapshot(value, 'telemetry.state');
    } catch (error) {
      if (!this.stateRejected) this.report('Telemetry state snapshot rejected', error);
      this.stateRejected = true;
      return undefined;
    }
  }

  /** Raw frames align samples to four bytes so the webview can use Float32Array directly. */
  raw(frame: ArrayBuffer): ControlTelemetry {
    const length = new DataView(frame).getUint32(0, true);
    if (length > frame.byteLength - 4) throw new Error('invalid telemetry header');
    const metadata = packedTelemetry(
      JSON.parse(new TextDecoder().decode(new Uint8Array(frame, 4, length))),
      'telemetry',
    );
    const offset = (length + 7) & ~3;
    return this.unpack(metadata, new Float32Array(frame, offset));
  }

  sse(data: string): ControlTelemetry {
    const metadata = packedTelemetry(JSON.parse(data), 'telemetry');
    const bytes = Uint8Array.from(atob(metadata.meters.samples), (character) =>
      character.charCodeAt(0),
    );
    return this.unpack(metadata, new Float32Array(bytes.buffer));
  }

  private unpack(packet: PackedTelemetry, samples: Float32Array): ControlTelemetry {
    const bindings = packet.meters.bindings ?? this.bindings;
    if (!bindings) throw new Error('meter bindings missing');
    const decoded = unpack(packet, bindings, samples);
    this.bindings = bindings;
    return { ...decoded, state: this.state(packet.state) };
  }
}

function unpack(
  packet: PackedTelemetry,
  bindings: MeterBinding[],
  samples: Float32Array,
): ControlTelemetry {
  let offset = 0;
  const take = (count: number) => {
    if (!Number.isSafeInteger(count) || count < 0 || offset + count > samples.length)
      throw new Error('invalid meter binding');
    const values = Array.from(samples.subarray(offset, offset + count));
    offset += count;
    return values;
  };
  const nodes = bindings.map((binding) => {
    const peaks = take(binding.channels);
    const dynamics =
      binding.inputChannels === null
        ? undefined
        : {
            inputPeaks: take(binding.inputChannels),
            gainReductionDb: take(1)[0] as number,
            thresholdDb: take(1)[0] as number,
          };
    return {
      nodeId: binding.nodeId,
      portId: binding.portId,
      peaks,
      ...(dynamics ? { dynamics } : {}),
    };
  });
  if (offset !== samples.length || samples.some((value) => !Number.isFinite(value)))
    throw new Error('invalid meter samples');
  return {
    engineIncarnation: packet.engineIncarnation,
    sequence: packet.sequence,
    meters: {
      protocolVersion: packet.meters.protocolVersion,
      revision: packet.meters.revision,
      nodes,
    },
  };
}
