import type { NativeSampleFormat } from '../../../core/domain/control/nativeFormat';
import type { VirtualEndpointSpec } from '../../../core/domain/control/status';
import type { DriverAction } from '../../../core/domain/driver';

export interface EndpointConfiguration {
  displayName: string;
  sampleRate?: number;
  sampleFormat?: NativeSampleFormat;
  renderChannels: number;
  captureChannels: number;
}

function resizedChannels(stream: VirtualEndpointSpec['render'], channels: number) {
  if (!stream || channels === stream.channels) return stream;
  return {
    channels,
    channelLayout:
      channels === 1
        ? ('mono' as const)
        : channels === 2
          ? ('stereo' as const)
          : ('discrete' as const),
  };
}

export function configuredDevice(
  spec: VirtualEndpointSpec,
  update: EndpointConfiguration,
): DriverAction {
  return {
    action: 'configure',
    spec: {
      ...spec,
      displayName: update.displayName,
      defaultSampleRate: update.sampleRate ?? spec.defaultSampleRate,
      render: resizedChannels(spec.render, update.renderChannels),
      capture: resizedChannels(spec.capture, update.captureChannels),
    },
  };
}
