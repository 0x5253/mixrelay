import type { TFunction } from 'i18next';
import { useCallback } from 'react';
import type { AudioGraphChangeKind } from '../../core/audio-graph/AudioGraphCanvas';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import type {
  ExternalProcessorDescriptor,
  ProviderNodeConfig,
  ProviderPorts,
} from '../../core/domain/control/processors';
import type { NodeChoiceControl } from '../../core/ui/AudioNodeCard';
import { changeProcessorLayout, type GraphPresentation } from './model';
import {
  processorLayoutDescription,
  processorLayoutLabel,
  selectableProcessorLayouts,
} from './processorCatalogMenu';

function providerConfig(node: AudioGraphModel['nodes'][number] | undefined) {
  return node?.runtime?.type === 'provider'
    ? (node.runtime.config as ProviderNodeConfig | undefined)
    : undefined;
}

function layoutOptions(layouts: readonly ProviderPorts[], t: TFunction) {
  const labels = layouts.map((layout) => processorLayoutLabel(layout, t));
  return layouts.map((layout, index) => {
    const label = labels[index] as string;
    const repeats = labels.filter((other) => other === label).length;
    const ordinal = labels.slice(0, index).filter((other) => other === label).length + 1;
    return {
      value: layout.layoutId,
      label: repeats > 1 ? `${label} · ${ordinal}` : label,
      description: processorLayoutDescription(layout, t),
    };
  });
}

/** Offers a plug-in node's other catalog layouts in its Options; nodes with one layout,
 * or whose saved layout the catalog no longer offers, get no control. */
export function presentProcessorLayouts(
  model: AudioGraphModel,
  processors: readonly ExternalProcessorDescriptor[],
  t: TFunction,
): AudioGraphModel {
  const byType = new Map(processors.map((processor) => [processor.typeId, processor]));
  let changed = false;
  const nodes = model.nodes.map((node) => {
    const config = providerConfig(node);
    const processor = config ? byType.get(config.typeId) : undefined;
    const value = config?.ports?.layoutId;
    if (!processor || !value) return node;
    const layouts = selectableProcessorLayouts(processor);
    if (layouts.length < 2 || !layouts.some((layout) => layout.layoutId === value)) return node;
    changed = true;
    const channelLayoutControl: NodeChoiceControl = {
      label: t('routing.processorLayouts.title'),
      value,
      options: layoutOptions(layouts, t),
    };
    return { ...node, appearance: { ...node.appearance, channelLayoutControl } };
  });
  return changed ? { ...model, nodes } : model;
}

/** Applies a layout chosen on a node as one runtime edit (one undo step). */
export function useProcessorLayoutChange(
  model: AudioGraphModel | null,
  processors: readonly ExternalProcessorDescriptor[],
  presentation: GraphPresentation,
  updateModel: (model: AudioGraphModel, kind: AudioGraphChangeKind) => void,
) {
  return useCallback(
    (nodeId: string, layoutId: string) => {
      if (!model) return;
      const config = providerConfig(model.nodes.find((node) => node.id === nodeId));
      const processor = processors.find((candidate) => candidate.typeId === config?.typeId);
      const ports = processor
        ? selectableProcessorLayouts(processor).find((layout) => layout.layoutId === layoutId)
        : undefined;
      if (!processor || !ports) return;
      const next = changeProcessorLayout(model, nodeId, processor, ports, presentation);
      // Reselecting the current layout is not an edit and must not reapply the graph.
      if (next !== model) updateModel(next, 'runtime');
    },
    [model, processors, presentation, updateModel],
  );
}
