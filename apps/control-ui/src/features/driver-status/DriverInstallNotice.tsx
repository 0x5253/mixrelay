import { useTranslation } from 'react-i18next';
import type { DriverState } from '../../core/domain/control/status';
import { ActionButton } from '../../core/ui/ActionButton';
import styles from './DriverInstallNotice.module.css';

export function DriverInstallNotice({
  state,
  onInstall,
  className,
}: {
  state?: DriverState;
  onInstall?: () => void;
  className?: string;
}) {
  const { t } = useTranslation();
  if (state !== 'not_installed') return null;
  return (
    <aside className={`${styles.root} ${className ?? ''}`} aria-label={t('driver.title')}>
      <p>{t('driver.installNotice')}</p>
      {onInstall ? (
        <ActionButton onClick={onInstall}>{t('driver.getInstaller')}</ActionButton>
      ) : null}
    </aside>
  );
}
