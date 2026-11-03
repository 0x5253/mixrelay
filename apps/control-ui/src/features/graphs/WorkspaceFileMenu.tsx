import { useEffect, useRef } from 'react';
import { useTranslation } from 'react-i18next';
import type { WorkspacePort } from '../../core/ports/control/WorkspacePort';
import { HierarchicalMenuButton } from '../../core/ui/HierarchicalMenu';

/** The engine capabilities this module uses. */
type WorkspaceFilePort = Pick<WorkspacePort, 'workspaceFile' | 'onWorkspaceFile'>;

type Action = 'new' | 'open' | 'save' | 'save_as';
export function WorkspaceFileMenu({
  controlPort,
  disabled,
  run,
}: {
  controlPort: WorkspaceFilePort;
  disabled: boolean;
  run: (action: Action, locale: string) => Promise<void>;
}) {
  const { i18n, t } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const current = useRef({ disabled, run, locale });
  current.current = { disabled, run, locale };
  useEffect(() => {
    let disposed = false;
    let stop: (() => void) | undefined;
    void controlPort
      .onWorkspaceFile?.((action) => {
        const value = current.current;
        if (!value.disabled) void value.run(action, value.locale);
      })
      .then((unsubscribe) => {
        if (disposed) unsubscribe();
        else stop = unsubscribe;
      })
      .catch((error: unknown) => console.error('Workspace menu listener failed', error));
    return () => {
      disposed = true;
      stop?.();
    };
  }, [controlPort]);
  if (!controlPort.workspaceFile) return null;
  return (
    <HierarchicalMenuButton
      label={t('workspaceFile.file')}
      buttonContent={t('workspaceFile.file')}
      items={(
        [
          ['new', t('workspaceFile.new')],
          ['open', t('workspaceFile.open')],
          ['save', t('workspaceFile.save')],
          ['save_as', t('workspaceFile.saveAs')],
        ] as const
      ).map(([id, label]) => ({ id, label, disabled }))}
      onSelect={(id) => {
        if (!disabled && (id === 'new' || id === 'open' || id === 'save' || id === 'save_as'))
          void run(id, locale);
      }}
    />
  );
}
