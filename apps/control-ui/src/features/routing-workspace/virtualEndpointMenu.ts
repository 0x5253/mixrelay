import type { TFunction } from 'i18next';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  AudioSessionStatus,
  VirtualEndpointSettings,
  VirtualEndpointSpec,
} from '../../core/domain/control/status';
import type { HierarchicalMenuItem } from '../../core/ui/HierarchicalMenu';
import { type RoutingNodeRequest, virtualEndpointIdentity } from './model';

export type VirtualNodeSide = 'input' | 'output';

function referenceChannels(
  side: VirtualNodeSide,
  spec: Pick<VirtualEndpointSpec, 'render' | 'capture'>,
) {
  return side === 'input' ? (spec.capture?.channels ?? 0) : (spec.render?.channels ?? 0);
}

export function virtualEndpointItemId(side: VirtualNodeSide, endpointId: string) {
  return `virtual/${side}/${encodeURIComponent(endpointId)}`;
}

export function virtualEndpointNodeRequest(
  side: VirtualNodeSide,
  spec: VirtualEndpointSpec,
): RoutingNodeRequest | null {
  if (referenceChannels(side, spec) < 1) return null;
  return { kind: side === 'input' ? 'virtual_input' : 'virtual_output', spec };
}

type VirtualEndpointAction =
  | { kind: 'create'; side: VirtualNodeSide }
  | { kind: 'reference'; side: VirtualNodeSide; spec: VirtualEndpointSpec };

export function resolveVirtualEndpointAction(
  itemId: string,
  settings: VirtualEndpointSettings,
): VirtualEndpointAction | null {
  for (const side of ['input', 'output'] as const) {
    if (itemId === `virtual-new-${side}` && settings.canCreate && settings.creationTemplate)
      return { kind: 'create', side };
    const device = settings.devices.find(
      ({ spec }) => virtualEndpointItemId(side, spec.id) === itemId,
    );
    if (device) return { kind: 'reference', side, spec: device.spec };
  }
  return null;
}

export function virtualDeviceMenuItem(
  side: VirtualNodeSide,
  model: AudioGraphModel | null,
  t: TFunction,
  session?: AudioSessionStatus,
): HierarchicalMenuItem {
  const settings = session?.virtualEndpointSettings;
  const unavailable = session?.capabilities.virtualEndpoint !== true || !settings;
  const portSide = side === 'input' ? 'outputPorts' : 'inputPorts';
  return {
    id: `virtual-${side}`,
    label: t('routing.nodes.virtualDevice'),
    disabled: unavailable,
    children: [
      {
        id: `virtual-new-${side}`,
        label: t('routing.nodes.newVirtualDevice'),
        disabled: unavailable || !settings.canCreate || !settings.creationTemplate,
      },
      ...(settings?.devices.length
        ? [{ id: `virtual-${side}-divider`, kind: 'divider' as const, label: '' }]
        : []),
      ...(settings?.devices ?? []).map(({ spec }) => {
        const attached = model?.nodes.some(
          (node) => virtualEndpointIdentity(node) === spec.id && node[portSide].length > 0,
        );
        const channels = referenceChannels(side, spec);
        return {
          id: virtualEndpointItemId(side, spec.id),
          label: spec.displayName,
          description: [
            channels ? `${channels} CH` : null,
            attached ? t('routing.nodes.virtualAttached') : null,
          ]
            .filter(Boolean)
            .join(' · '),
          disabled: unavailable || attached || !channels,
        };
      }),
    ],
  };
}
