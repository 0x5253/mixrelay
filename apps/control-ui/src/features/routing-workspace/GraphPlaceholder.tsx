import { useTranslation } from 'react-i18next';
import styles from './RoutingWorkspace.module.css';
import type { WorkspaceState } from './WorkspaceProblems';

/** A failed layout read says so here; its retry lives with the other problems in diagnostics. */
export function GraphPlaceholder({
  state,
  layoutLoaded,
  layoutFailed,
}: {
  state: WorkspaceState;
  layoutLoaded: boolean;
  layoutFailed: boolean;
}) {
  const { t } = useTranslation();
  const placeholderState =
    state === 'unavailable' || layoutLoaded ? state : layoutFailed ? 'layoutFailed' : 'loading';
  return (
    <div
      key={placeholderState}
      className={styles.placeholder}
      role={placeholderState === 'unavailable' ? 'alert' : 'status'}
    >
      {t(`routing.states.${placeholderState}`)}
    </div>
  );
}
