import { type ReactNode, type Ref, useContext } from 'react';
import { useTranslation } from 'react-i18next';
import { MidiContext } from '../../core/midi/MidiContext';
import { TitleField } from '../../core/ui/TitleField';
import { WorkspaceHeader } from '../../core/ui/WorkspaceHeader';
import styles from './RoutingWorkspace.module.css';

export function WorkspaceToolbar({
  titleRef,
  title,
  onTitleChange,
  headerActions,
  headerEnd,
}: {
  titleRef: Ref<HTMLInputElement>;
  title?: string;
  onTitleChange?: (title: string) => void;
  headerActions?: ReactNode;
  headerEnd?: ReactNode;
}) {
  const { t } = useTranslation();
  const mapping = useContext(MidiContext)?.active ?? false;
  return (
    <WorkspaceHeader>
      <div className={styles.titleBlock}>
        <TitleField
          ref={titleRef}
          aria-label={t('studio.graphName')}
          readOnly={mapping}
          disabled={mapping}
          onBlur={(event) => {
            const normalized = event.currentTarget.value.trim();
            onTitleChange?.(normalized || t('studio.untitledGraph'));
          }}
          onChange={(event) => onTitleChange?.(event.currentTarget.value)}
          onKeyDown={(event) => {
            if (event.key === 'Enter') event.currentTarget.blur();
          }}
          spellCheck={false}
          value={title ?? t('studio.untitledGraph')}
        />
      </div>
      {headerActions ? (
        <div
          className={styles.headerActions}
          inert={mapping}
          data-midi-inactive={mapping || undefined}
        >
          {headerActions}
        </div>
      ) : null}
      {headerEnd}
    </WorkspaceHeader>
  );
}
