import type { TFunction } from 'i18next';
import { useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import type { ProcessorEditorLabels } from '../../core/audio-graph/AudioGraphCanvas';
import { processorCatalogPlaceholder } from './processorCatalogMenu';
import { createNodeMenuItems } from './routingNodeMenus';
import type { WorkspaceViewProps } from './workspaceViewProps';

export function routingContextLabels(t: TFunction) {
  return {
    renameNode: t('routing.nodeName.title'),
    connectChannels: t('routing.context.connectChannels'),
    removeNode: t('routing.context.removeNode'),
    removeSelection: t('routing.context.removeSelection'),
    disconnect: t('routing.context.disconnect'),
    disconnectPort: t('routing.context.disconnectPort'),
    disconnectPorts: t('routing.context.disconnectPorts'),
    removeCable: t('routing.context.removeCable'),
    fitView: t('routing.context.fitView'),
    selectAll: t('routing.context.selectAll'),
    clearSelection: t('routing.context.clearSelection'),
  };
}

export function useProcessorEditorLabels() {
  const { t } = useTranslation();
  return useMemo<ProcessorEditorLabels>(
    () => ({
      open: t('routing.processorEditor.open'),
      retry: t('routing.processorControl.retry'),
      failure: {
        display_unavailable: t('routing.processorEditor.failures.displayUnavailable'),
        host_window_failed: t('routing.processorEditor.failures.hostWindowFailed'),
        no_editor: t('routing.processorEditor.failures.noEditor'),
        plugin_rejected: t('routing.processorEditor.failures.pluginRejected'),
        worker_unavailable: t('routing.processorEditor.failures.workerUnavailable'),
      },
    }),
    [t],
  );
}

/** The add-node menu for the current inventory, with the processor scan summary. */
export function workspaceMenuItems(
  props: Pick<
    WorkspaceViewProps,
    'applications' | 'audioDevices' | 'audioSession' | 'model' | 'processorCatalog' | 'processors'
  >,
  t: TFunction,
) {
  return createNodeMenuItems({
    applications: props.applications,
    audioDevices: props.audioDevices,
    processors: props.processors,
    model: props.model,
    t,
    session: props.audioSession,
    processorEmptyLabel: t(processorCatalogPlaceholder(props.processorCatalog)),
    processorReportDescription: props.processorCatalog.failure
      ? t('routing.processorControl.incomplete')
      : props.processorCatalog.diagnostics.length
        ? t('routing.processorControl.details', {
            count: new Set(props.processorCatalog.diagnostics.map((item) => JSON.stringify(item)))
              .size,
          })
        : undefined,
  });
}
