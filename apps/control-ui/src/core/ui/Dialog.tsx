import { type ReactNode, useId } from 'react';
import { Dialog as AriaDialog, Button, Heading, Modal, ModalOverlay } from 'react-aria-components';
import { useTranslation } from 'react-i18next';
import styles from '../theme/ComponentTheme.module.css';
import { Tooltip } from './Tooltip';

/** React Aria owns the focus scope, keyboard dismissal and restoration. */
export function Dialog({
  children,
  title,
  opened,
  onClose,
  closeLabel,
  busy = false,
  size = 'sm',
  withCloseButton = true,
}: {
  children: ReactNode;
  title: string;
  opened: boolean;
  onClose: () => void;
  closeLabel?: string;
  busy?: boolean;
  size?: 'xs' | 'sm' | 'md';
  withCloseButton?: boolean;
}) {
  const { t } = useTranslation();
  const id = useId();
  return (
    <ModalOverlay
      style={{ zIndex: 'var(--layer-dialog)' }}
      className={styles.dialogOverlay}
      isOpen={opened}
      isDismissable={!busy}
      isKeyboardDismissDisabled={busy}
      // The owner controls opening. This overlay only requests dismissal.
      onOpenChange={onClose}
    >
      <Modal className={styles.dialogContent} data-size={size}>
        <AriaDialog
          className={styles.dialog}
          render={(props) => <section {...props} role="dialog" aria-modal="true" />}
          aria-labelledby={id}
        >
          <header className={styles.dialogHeader}>
            <Heading slot="title" id={id} className={styles.dialogTitle}>
              {title}
            </Heading>
            {withCloseButton ? (
              <Tooltip label={closeLabel ?? t('common.cancel')}>
                <Button
                  className={styles.closeButton}
                  isDisabled={busy}
                  aria-label={closeLabel ?? t('common.cancel')}
                  onPress={onClose}
                >
                  ×
                </Button>
              </Tooltip>
            ) : null}
          </header>
          <div className={styles.dialogBody}>{children}</div>
        </AriaDialog>
      </Modal>
    </ModalOverlay>
  );
}
