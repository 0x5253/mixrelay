import type { TFunction } from 'i18next';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  VirtualEndpointSettings,
  VirtualEndpointSpec,
} from '../../core/domain/control/status';
import type { EndpointControl } from '../../core/ui/AudioNodeEndpointOptions';
import type { NodeRuntime } from './runtimeAudioNode';
import type { VirtualEndpointUpdate } from './virtualEndpointNodes';

/** The Driver transaction that owns virtual endpoint settings outside the graph. */
export interface DeviceDriverAction {
  settings?: VirtualEndpointSettings;
  enabled: boolean;
  error: { endpointId: string | null; message: string } | null;
  change: (endpointId: string, update: VirtualEndpointUpdate) => void;
  create: (spec: VirtualEndpointSpec) => Promise<VirtualEndpointSpec | null>;
}

function driverEndpointControl(
  control: EndpointControl,
  device: VirtualEndpointSettings['devices'][number] | undefined,
  action: DeviceDriverAction,
  endpointError: string | null,
  t: TFunction,
): EndpointControl {
  const editable = device && action.enabled;
  return {
    ...control,
    displayName: device?.spec.displayName ?? control.displayName,
    renderChannels: device?.spec.render?.channels ?? control.renderChannels,
    captureChannels: device?.spec.capture?.channels ?? control.captureChannels,
    canRename: Boolean(editable && action.settings?.canRename),
    allowedRenderChannels: editable ? action.settings?.renderChannels : [],
    allowedCaptureChannels: editable ? action.settings?.captureChannels : [],
    observedRate: device
      ? { label: t('audioSession.sampleRate'), value: device.spec.defaultSampleRate }
      : undefined,
    sampleRateOptions: editable ? device.spec.supportedSampleRates : undefined,
    rateError: endpointError,
    channelsLabel: t('driverManagement.channels'),
    nativeFormats: device?.nativeFormats,
    nativeFormatLabels: {
      bitDepth: t('driverManagement.bitDepth'),
      mixed: t('driverManagement.mixed'),
      unknown: t('driverManagement.unknown'),
    },
    formatsDisabled: !action.enabled,
    formatError: endpointError,
  };
}

/** Shows each virtual endpoint with the Driver's current device spec and what it lets the user edit. */
export function presentDriverEndpointControls(
  model: AudioGraphModel,
  action: DeviceDriverAction | undefined,
  t: TFunction,
): AudioGraphModel {
  if (!action) return model;
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      const control = node.appearance.endpointControl;
      if (!control) return node;
      // Endpoint controls are constructed only after the native endpoint spec is validated.
      const id = ((node.runtime as NodeRuntime).config as { id: string }).id;
      const device = action.settings?.devices.find((candidate) => candidate.spec.id === id);
      const endpointError =
        action.error && action.error.endpointId === id ? action.error.message : null;
      return {
        ...node,
        appearance: {
          ...node.appearance,
          title: device?.spec.displayName ?? node.appearance.title,
          endpointControl: driverEndpointControl(control, device, action, endpointError, t),
        },
      };
    }),
  };
}
