import type { TFunction } from 'i18next';
import { useCallback, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { PlaceGraphNodes } from '../../core/audio-graph/nodePlacement';
import type { ExternalProcessorDiagnostic } from '../../core/domain/control/processors';

import { ActionButton } from '../../core/ui/ActionButton';
import { Dialog } from '../../core/ui/Dialog';
import { DisclosureList, type DisclosureListItem } from '../../core/ui/DisclosureList';
import { Inline } from '../../core/ui/Inline';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';
import styles from './ProcessorScanReport.module.css';
import { PROCESSOR_SCAN_REPORT_ITEM_ID } from './routingNodeMenus';
import type { useProcessorCatalog } from './useProcessorCatalog';

export function useProcessorScanReport(
  catalog: ReturnType<typeof useProcessorCatalog>,
  addNode: (
    itemId: string,
    position: { x: number; y: number },
    placeNodes?: PlaceGraphNodes,
  ) => void,
) {
  const [opened, setOpened] = useState(false);
  const onMenuAction = useCallback(
    (itemId: string, position: { x: number; y: number }, placeNodes?: PlaceGraphNodes) => {
      if (itemId === PROCESSOR_SCAN_REPORT_ITEM_ID) setOpened(true);
      else addNode(itemId, position, placeNodes);
    },
    [addNode],
  );
  return {
    onMenuAction,
    dialog: (
      <ProcessorScanReport catalog={catalog} opened={opened} onClose={() => setOpened(false)} />
    ),
  };
}

export function ProcessorScanReport({
  catalog,
  opened,
  onClose,
}: {
  catalog: ReturnType<typeof useProcessorCatalog>;
  opened: boolean;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <Dialog
      opened={opened}
      onClose={onClose}
      title={t('routing.processorControl.reportTitle')}
      size="md"
      closeLabel={t('common.close')}
    >
      <ProcessorScanReportContent catalog={catalog} />
    </Dialog>
  );
}

export function ProcessorScanReportContent({
  catalog,
}: {
  catalog: ReturnType<typeof useProcessorCatalog>;
}) {
  const { t } = useTranslation();
  const diagnostics = new Map(catalog.diagnostics.map((item) => [JSON.stringify(item), item]));
  return (
    <Stack gap="section">
      <Text>
        {t(
          `routing.processorControl.${catalog.busy ? 'scanning' : (catalog.failure ?? (diagnostics.size ? 'partial' : 'complete'))}`,
        )}
      </Text>
      <Inline>
        <ActionButton
          emphasis="outlined"
          disabled={catalog.busy}
          onClick={catalog.failure ? catalog.retry : () => catalog.refresh(false)}
        >
          {t(catalog.failure ? 'routing.processorControl.retry' : 'routing.rescanProcessors')}
        </ActionButton>
        {diagnostics.size > 0 && (
          <Text kind="supporting">
            {t('routing.processorControl.details', { count: diagnostics.size })}
          </Text>
        )}
      </Inline>
      {diagnostics.size > 0 && (
        <DisclosureList
          items={[...diagnostics].map(([key, diagnostic]) =>
            scanDiagnosticItem(key, diagnostic, t),
          )}
        />
      )}
    </Stack>
  );
}

function scanDiagnosticItem(
  id: string,
  diagnostic: ExternalProcessorDiagnostic,
  t: TFunction,
): DisclosureListItem {
  const title = [
    diagnostic.format.toUpperCase(),
    diagnostic.pluginName,
    t(`routing.processorControl.${diagnostic.stage}`),
  ]
    .filter(Boolean)
    .join(' · ');
  return {
    id,
    title,
    content: (
      <Stack>
        <Text kind="supporting">{t(`routing.processorControl.${diagnostic.stage}Failed`)}</Text>
        <pre className={styles.technicalDetails}>
          {diagnostic.pluginId ? `${diagnostic.pluginId}\n` : ''}
          {diagnostic.message}
        </pre>
      </Stack>
    ),
  };
}
