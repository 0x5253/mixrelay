import type {
  AudioDeviceDescriptor,
  CapturableApplication,
} from '../../core/domain/control/inventory';
import type { ExternalProcessorDescriptor } from '../../core/domain/control/processors';
import { applicationInventorySelection } from './applicationIdentity';
import { isChainKind, isEffectKind } from './builtinEffects';
import type { RoutingNodeRequest } from './model';
import { processorSelectionForMenuItem } from './processorCatalogMenu';
import {
  audioInputItemId,
  audioOutputItemId,
  SYSTEM_AUDIO_EXCLUDED_BUNDLE_IDS,
  SYSTEM_AUDIO_ITEM_ID,
} from './routingNodeMenus';

/** What the add-node menu can offer at the moment an item is chosen. */
export interface RoutingNodeInventory {
  applications: readonly CapturableApplication[];
  audioDevices: readonly AudioDeviceDescriptor[];
  processors: readonly ExternalProcessorDescriptor[];
  /** Present only where the backend can capture all system audio; holds its node title. */
  systemAudioTitle?: string;
}

/**
 * Resolves an add-node menu item to the node it creates. A selection captured before the
 * inventory changed resolves to `null` rather than a guess.
 */
export function routingNodeRequest(
  itemId: string,
  inventory: RoutingNodeInventory,
): RoutingNodeRequest | null {
  if (itemId === SYSTEM_AUDIO_ITEM_ID) {
    return inventory.systemAudioTitle === undefined
      ? null
      : {
          kind: 'application',
          title: inventory.systemAudioTitle,
          selector: { match: 'all_except', bundle_ids: [...SYSTEM_AUDIO_EXCLUDED_BUNDLE_IDS] },
        };
  }
  for (const application of inventory.applications) {
    const selection = applicationInventorySelection(application);
    if (selection?.id === itemId)
      return { kind: 'application', selector: selection.selector, title: application.displayName };
  }
  const input = inventory.audioDevices.find((device) => audioInputItemId(device) === itemId);
  if (input)
    return {
      channels: supportedChannelCount(input.inputChannels),
      kind: 'audio_input',
      title: input.displayName,
      uid: input.uid,
    };
  const output = inventory.audioDevices.find((device) => audioOutputItemId(device) === itemId);
  if (output)
    return {
      channels: supportedChannelCount(output.outputChannels),
      kind: 'audio_output',
      title: output.displayName,
      uid: output.uid,
    };
  const fixed = fixedNodeRequest(itemId);
  if (fixed) return fixed;
  const processor = processorSelectionForMenuItem(itemId, inventory.processors);
  return processor ? { kind: 'provider', ...processor } : null;
}

function fixedNodeRequest(itemId: string): RoutingNodeRequest | null {
  if (itemId === 'compressor' || itemId === 'limiter' || itemId === 'recorder')
    return { kind: itemId };
  const effect = itemId.startsWith('builtin:') ? itemId.slice('builtin:'.length) : undefined;
  if (isEffectKind(effect)) return { kind: 'effect', effect };
  const chain = itemId.startsWith('effect-chain:')
    ? itemId.slice('effect-chain:'.length)
    : undefined;
  if (isChainKind(chain)) return { kind: 'effect_chain', preset: chain };
  return null;
}

/** Devices report any channel count; a node supports 1 to 64. */
export function supportedChannelCount(channels: number): number {
  return Math.max(1, Math.min(64, Math.trunc(channels)));
}
