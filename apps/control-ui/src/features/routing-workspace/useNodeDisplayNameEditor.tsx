import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { GraphLayoutController } from '../../core/application/workspace/GraphLayoutController';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { NameDialog } from '../../core/ui/NameDialog';
import type { useGraphLayout } from './useGraphLayout';

export function presentNodeDisplayNames(model: AudioGraphModel, labels?: Record<string, string>) {
  const names = new Map(Object.entries(labels ?? {}));
  const nodes = model.nodes.map((node) => {
    const displayName = names.get(node.id);
    return node.displayName === displayName ? node : { ...node, displayName };
  });
  return nodes.every((node, index) => node === model.nodes[index]) ? model : { ...model, nodes };
}

/** This owner writes presentation metadata only; it never submits an audio document. */
export function useNodeDisplayNameEditor(
  model: AudioGraphModel | null,
  layout: ReturnType<typeof useGraphLayout>,
  disabled: boolean,
) {
  const { t } = useTranslation();
  const [selected, setSelected] = useState<{
    nodeId: string;
    owner: GraphLayoutController;
  } | null>(null);
  const node =
    selected?.owner === layout.controller
      ? model?.nodes.find((candidate) => candidate.id === selected.nodeId)
      : undefined;
  const enabled = layout.controller.enabled && layout.loaded && !layout.loading && !disabled;
  const request = enabled
    ? (nodeId: string) => setSelected({ nodeId, owner: layout.controller })
    : undefined;
  const dialog =
    node && enabled ? (
      <NameDialog
        key={node.id}
        title={t('routing.nodeName.title')}
        nameLabel={t('routing.nodeName.label')}
        placeholder={node.appearance.title}
        initialName={new Map(Object.entries(layout.layout.nodeLabels ?? {})).get(node.id) ?? ''}
        createLabel={t('routing.nodeName.save')}
        allowEmpty
        maxLength={128}
        enabled
        onCancel={() => setSelected(null)}
        onCreate={(value) => {
          layout.updateNodeLabel(node.id, value === node.appearance.title ? '' : value);
          setSelected(null);
        }}
      />
    ) : null;
  return { request, dialog };
}
