import { useTranslation } from 'react-i18next';
import { ActionButton } from '../core/ui/ActionButton';
import {
  hasPlatformModifier,
  isNativeTextEntryTarget,
  platformModifier,
} from '../core/ui/platformShortcut';
import { GraphList } from '../features/graphs/GraphList';
import type { useGraphCatalog } from '../features/graphs/useGraphCatalog';
import styles from './StudioScreen.module.css';
import { catalogErrorText, graphListHistoryAction } from './studioRules';

export interface GraphSidebarProps {
  hidden: boolean;
  collection: ReturnType<typeof useGraphCatalog>;
  disabled: boolean;
  onCreate: () => void;
  onSelect: (id: string) => void;
}

export function GraphSidebar({
  hidden,
  collection,
  disabled,
  onCreate,
  onSelect,
}: GraphSidebarProps) {
  const { t } = useTranslation();
  const { catalog, busy: catalogBusy, error: catalogError, reload: reloadCatalog } = collection;
  const errorText = catalogErrorText(t, catalogError);
  return (
    <aside
      onKeyDown={(event) => {
        if (disabled) return;
        const action = graphListHistoryAction(
          event,
          hasPlatformModifier(event),
          isNativeTextEntryTarget(event.target),
        );
        if (!action) return;
        event.preventDefault();
        event.stopPropagation();
        void collection.mutate({ action });
      }}
      id="graph-sidebar"
      className={styles.sidebar}
      aria-label={t('studio.graphs')}
      hidden={hidden}
    >
      <nav className={styles.graphList} aria-label={t('studio.graphs')}>
        {catalog ? (
          <GraphList
            catalog={catalog}
            disabled={disabled}
            busy={catalogBusy}
            error={errorText}
            onSelect={onSelect}
            onDelete={(id) => {
              if (!disabled) void collection.mutate({ action: 'delete', id });
            }}
          />
        ) : null}
        {catalogError ? (
          <div role="alert">
            <p>{errorText}</p>
            <ActionButton disabled={catalogBusy} onClick={() => void reloadCatalog()}>
              {t('studio.reloadGraphs')}
            </ActionButton>
          </div>
        ) : null}
        {catalog?.persistenceError ? <p role="alert">{t('studio.graphSaveFailed')}</p> : null}
        <div className={styles.sidebarFooter}>
          <ActionButton
            density="compact"
            stretch
            align="start"
            aria-keyshortcuts={`${platformModifier()}+Shift+N`}
            disabled={!catalog}
            onClick={onCreate}
          >
            {/* One inline run: the flex button would drop a space between children. */}
            <span>
              <span aria-hidden="true">+ </span>
              {t('studio.newGraph')}
            </span>
          </ActionButton>
        </div>
      </nav>
    </aside>
  );
}
