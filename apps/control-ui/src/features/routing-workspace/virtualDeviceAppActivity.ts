import type { TFunction } from 'i18next';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { DeviceAppActivity } from '../../core/domain/control/inventory';
import type { VirtualEndpointSettings } from '../../core/domain/control/status';
import type { DeviceAppActivityProps } from '../../core/ui/DeviceAppActivity';
import type { DeviceActivityObservation } from './application/DeviceActivitySession';
import { virtualEndpointIdentity } from './model';

/** Bundle identity spans processes/devices; unbundled clients are identified only by PID. */
function distinctApps(records: DeviceAppActivity[]) {
  const apps = new Map<string, DeviceAppActivity>();
  for (const app of records) {
    const key = app.bundleId ? `bundle:${app.bundleId}` : `pid:${app.processId}`;
    const previous = apps.get(key);
    apps.set(
      key,
      previous
        ? {
            ...previous,
            iconDataUrl: previous.iconDataUrl || app.iconDataUrl,
            playing: previous.playing || app.playing,
            recording: previous.recording || app.recording,
          }
        : app,
    );
  }
  return [...apps.entries()].sort(([a], [b]) => (a < b ? -1 : 1));
}

function activityPresentation(
  direction: 'playing' | 'recording',
  device: string,
  uids: string[],
  observation: DeviceActivityObservation,
  t: TFunction,
  locale: string,
): DeviceAppActivityProps {
  const apps =
    observation.state === 'observed'
      ? distinctApps(uids.flatMap((uid) => observation.devices[uid] ?? [])).filter(
          ([, app]) => app[direction],
        )
      : [];
  const names = (values: typeof apps) =>
    new Intl.ListFormat(locale === 'pseudo' ? 'en' : locale, {
      style: 'long',
      type: 'conjunction',
    }).format(values.map(([, app]) => app.displayName));
  const emptyKey = direction === 'playing' ? 'emptyPlaying' : 'emptyRecording';
  const label =
    observation.state === 'observed'
      ? t('routing.deviceApps.empty')
      : t(`routing.deviceApps.${observation.state}`);
  const summary =
    observation.state !== 'observed'
      ? label
      : apps.length
        ? t(`routing.deviceApps.${direction}Summary`, { device, apps: names(apps) })
        : t(`routing.deviceApps.${emptyKey}`, { device });
  return {
    direction,
    summary,
    label,
    apps: apps.map(([id, app]) => ({
      id,
      iconUrl: app.iconDataUrl,
      label: t(`routing.deviceApps.${direction}App`, { app: app.displayName, device }),
    })),
    // A bare count reads the same in every language and never outgrows the card.
    overflowLabel: `+${Math.max(0, apps.length - 3)}`,
    overflowDescription: t(
      `routing.deviceApps.${direction === 'playing' ? 'morePlaying' : 'moreRecording'}`,
      { count: Math.max(0, apps.length - 3), device, apps: names(apps.slice(3)) },
    ),
  };
}

export function presentVirtualDeviceAppActivity(
  model: AudioGraphModel,
  settings: VirtualEndpointSettings | undefined,
  observation: DeviceActivityObservation,
  t: TFunction,
  locale: string,
): AudioGraphModel {
  return {
    ...model,
    nodes: model.nodes.map((node) => {
      if (node.runtime?.type !== 'virtual_endpoint') return node;
      const direction = node.runtime.binding?.side === 'source' ? 'playing' : 'recording';
      const endpoint = settings?.devices.find(
        ({ spec }) => spec.id === virtualEndpointIdentity(node),
      );
      const uids = endpoint?.nativeDeviceIds.filter(Boolean) ?? [];
      const observed =
        uids.length && node.runtime.binding ? observation : { state: 'unavailable' as const };
      const activity = activityPresentation(
        direction,
        endpoint?.spec.displayName ?? node.appearance.title,
        uids,
        observed,
        t,
        locale,
      );
      return { ...node, appearance: { ...node.appearance, deviceAppActivity: activity } };
    }),
  };
}
