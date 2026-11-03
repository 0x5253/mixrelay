import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import type { DriverState } from '../../core/domain/control/status';
import { getDriverPresentation } from '../../core/driver/presentation';
import { ActionButton } from '../../core/ui/ActionButton';
import { Panel } from '../../core/ui/Panel';
import { StatusBadge } from '../../core/ui/StatusBadge';

interface DriverStatusCardProps {
  state: DriverState;
  onAction?: () => void;
  actionLabel?: string;
  children?: ReactNode;
  busy?: boolean;
}

export function DriverStatusCard({
  onAction,
  state,
  actionLabel,
  children,
  busy,
}: DriverStatusCardProps) {
  const { t } = useTranslation();
  const presentation = getDriverPresentation(state);
  const isBusy = busy || state === 'installing';

  return (
    <Panel
      title={t('driver.title')}
      trailing={
        <StatusBadge tone={presentation.tone}>
          {t(`driver.states.${presentation.label}`)}
        </StatusBadge>
      }
    >
      {children}
      {onAction ? (
        <ActionButton disabled={isBusy} loading={isBusy} onClick={onAction}>
          {actionLabel ?? t(`driver.${presentation.action}`)}
        </ActionButton>
      ) : null}
    </Panel>
  );
}
