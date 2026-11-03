import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { GraphCatalog } from '../../core/domain/control/catalog';
import { CollectionItem } from '../../core/ui/CollectionItem';
import { ConfirmationDialog } from '../../core/ui/ConfirmationDialog';
import styles from './GraphList.module.css';

interface GraphListProps {
  catalog: GraphCatalog;
  disabled: boolean;
  busy?: boolean;
  error: string | null;
  onSelect: (id: string) => void;
  onDelete: (id: string) => void;
}

export function GraphList({
  catalog,
  disabled,
  busy = false,
  error,
  onSelect,
  onDelete,
}: GraphListProps) {
  const { t } = useTranslation();
  const [target, setTarget] = useState<string | null>(null);
  const graph = catalog.graphs.find((entry) => entry.id === target);
  useEffect(() => {
    if (!graph) setTarget(null);
  }, [graph]);
  return (
    <>
      {catalog.graphs.map((entry) => (
        <CollectionItem
          key={entry.id}
          name={entry.name}
          selected={entry.id === catalog.activeId}
          disabled={disabled}
          removeLabel={t('studio.deleteGraph')}
          onSelect={() => onSelect(entry.id)}
          onRemove={() => setTarget(entry.id)}
        />
      ))}
      <ConfirmationDialog
        opened={Boolean(graph)}
        onCancel={() => {
          if (!busy) setTarget(null);
        }}
        title={t('studio.deleteGraph')}
        cancelLabel={t('common.cancel')}
        busy={busy}
        disabled={disabled || !graph}
        onConfirm={() => {
          if (graph) onDelete(graph.id);
        }}
      >
        <p className={styles.notice}>{t('studio.deleteGraphNotice', { name: graph?.name })}</p>
        {error ? <p role="alert">{error}</p> : null}
      </ConfirmationDialog>
    </>
  );
}
