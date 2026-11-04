import type {
  ProviderBus,
  ProviderBusKind,
  ProviderChannelFormat,
  ProviderPorts,
} from '../../../core/domain/control/processors';
import { objectValue } from './configValues';

export function providerPortsFrom(value: unknown): ProviderPorts | null {
  const config = objectValue(value);
  if (
    !config ||
    !hasOnlyKeys(config, ['layoutId', 'inputs', 'outputs']) ||
    typeof config.layoutId !== 'string' ||
    !validProviderIdentity(config.layoutId, 256) ||
    !Array.isArray(config.inputs) ||
    !Array.isArray(config.outputs) ||
    config.inputs.length === 0 ||
    config.outputs.length === 0 ||
    config.inputs.length > 64 ||
    config.outputs.length > 64
  )
    return null;
  const inputs = config.inputs.map(providerBusFrom);
  const outputs = config.outputs.map(providerBusFrom);
  if (inputs.some((bus) => bus === null) || outputs.some((bus) => bus === null)) return null;
  const validInputs = inputs as ProviderBus[];
  const validOutputs = outputs as ProviderBus[];
  if (
    totalBusChannels(validInputs) > 64 ||
    totalBusChannels(validOutputs) > 64 ||
    !uniqueBusIds(validInputs) ||
    !uniqueBusIds(validOutputs)
  )
    return null;
  return { inputs: validInputs, layoutId: config.layoutId, outputs: validOutputs };
}

function providerBusFrom(value: unknown): ProviderBus | null {
  const bus = objectValue(value);
  if (
    !bus ||
    !hasOnlyKeys(bus, ['id', 'name', 'kind', 'channels', 'channelFormat']) ||
    typeof bus.id !== 'string' ||
    typeof bus.name !== 'string' ||
    !validProviderIdentity(bus.id, 128) ||
    byteLength(bus.name) > 1_024 ||
    bus.name.includes('\0') ||
    !['main', 'sidechain', 'auxiliary'].includes(String(bus.kind)) ||
    !['mono', 'stereo', 'surround5_1', 'surround7_1', 'discrete'].includes(
      String(bus.channelFormat),
    ) ||
    typeof bus.channels !== 'number' ||
    !Number.isInteger(bus.channels) ||
    bus.channels < 1 ||
    bus.channels > 64
  )
    return null;
  const expectedChannels = {
    mono: 1,
    stereo: 2,
    surround5_1: 6,
    surround7_1: 8,
  }[String(bus.channelFormat) as 'mono' | 'stereo' | 'surround5_1' | 'surround7_1'];
  if (expectedChannels !== undefined && bus.channels !== expectedChannels) return null;
  return {
    channelFormat: bus.channelFormat as ProviderChannelFormat,
    channels: bus.channels,
    id: bus.id,
    kind: bus.kind as ProviderBusKind,
    name: bus.name,
  };
}

function uniqueBusIds(buses: readonly ProviderBus[]) {
  const ids = buses.map((bus) => bus.id);
  return ids.every((id) => id.length > 0) && new Set(ids).size === ids.length;
}

function validProviderIdentity(value: string, maximumBytes: number) {
  return value.length > 0 && byteLength(value) <= maximumBytes && !/\p{Cc}/u.test(value);
}

function byteLength(value: string) {
  return new TextEncoder().encode(value).length;
}

function hasOnlyKeys(value: Record<string, unknown>, keys: readonly string[]) {
  return Object.keys(value).every((key) => keys.includes(key));
}

export function providerPortsMatch(left: ProviderPorts, right: ProviderPorts) {
  const sameBuses = (a: readonly ProviderBus[], b: readonly ProviderBus[]) =>
    a.length === b.length &&
    a.every((bus, index) => {
      const other = b[index];
      return (
        other !== undefined &&
        bus.id === other.id &&
        bus.kind === other.kind &&
        bus.channels === other.channels &&
        bus.channelFormat === other.channelFormat
      );
    });
  return (
    left.layoutId === right.layoutId &&
    sameBuses(left.inputs, right.inputs) &&
    sameBuses(left.outputs, right.outputs)
  );
}

export function totalBusChannels(buses: readonly ProviderBus[]) {
  return buses.reduce((total, bus) => total + bus.channels, 0);
}
