import type { AudioGraphModel } from '../../core/audio-graph/model';
import type { CapturableApplication } from '../../core/domain/control/inventory';
import { applicationInventorySelection, applicationSelectorKey } from './applicationIdentity';
import { objectValue } from './domain/configValues';

export function presentRoutingApplicationIcons(
  model: AudioGraphModel,
  applications: readonly CapturableApplication[],
): AudioGraphModel {
  const iconsBySelection = new Map<string, string | undefined>();
  for (const application of applications) {
    const selection = applicationInventorySelection(application);
    if (!selection) continue;
    const iconUrl = application.iconDataUrl || undefined;
    if (!iconsBySelection.has(selection.key) || (!iconsBySelection.get(selection.key) && iconUrl))
      iconsBySelection.set(selection.key, iconUrl);
  }
  let changed = false;
  const nodes = model.nodes.map((node) => {
    if (node.runtime?.type !== 'application_capture') return node;
    const config = objectValue(node.runtime.config);
    const selection =
      config?.requiresReselection === true ? undefined : applicationSelectorKey(config?.selector);
    const iconUrl = selection ? iconsBySelection.get(selection) : undefined;
    if (node.appearance.iconUrl === iconUrl) return node;
    changed = true;
    return { ...node, appearance: { ...node.appearance, iconUrl } };
  });
  return changed ? { ...model, nodes } : model;
}
