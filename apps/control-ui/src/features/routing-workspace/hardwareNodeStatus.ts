import type { TFunction } from 'i18next';
import type { AudioGraphModel, AudioGraphNode } from '../../core/audio-graph/model';
import type { AudioDeviceDescriptor } from '../../core/domain/control/inventory';

/** Presence is a display projection, never a graph edit or a default-device substitution. */
export function presentHardwareNodeStatus(
  model: AudioGraphModel,
  devices: readonly AudioDeviceDescriptor[],
  observed: boolean,
  t: TFunction,
): AudioGraphModel {
  if (!observed) return model;
  const inventory = new Map(devices.map((device) => [device.uid, device]));
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      const uid = hardwareUid(node);
      if (!uid) return node;
      const device = inventory.get(uid);
      const channels =
        node.runtime?.type === 'audio_device_input'
          ? device?.inputChannels
          : device?.outputChannels;
      if (channels && channels > 0) return node;
      return {
        ...node,
        appearance: {
          ...node.appearance,
          status: t('routing.nodeStatus.offline'),
          statusTone: 'neutral' as const,
          offline: true,
        },
      };
    }),
  };
}

function hardwareUid(node: AudioGraphNode): string | null {
  const runtime = node.runtime;
  if (runtime?.type !== 'audio_device_input' && runtime?.type !== 'audio_device_output')
    return null;
  const config = runtime.config;
  if (typeof config !== 'object' || config === null || !('selector' in config)) return null;
  const selector = config.selector;
  if (
    typeof selector !== 'object' ||
    selector === null ||
    !('match' in selector) ||
    selector.match !== 'persistent_uid'
  )
    return null;
  return 'uid' in selector && typeof selector.uid === 'string' ? selector.uid : null;
}
