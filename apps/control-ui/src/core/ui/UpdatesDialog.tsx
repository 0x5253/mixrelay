import type { ReactNode } from 'react';
import { Dialog } from './Dialog';

export function UpdatesDialog({
  children,
  closeLabel,
  onClose,
  opened,
  title,
}: {
  children: ReactNode;
  closeLabel: string;
  onClose: () => void;
  opened: boolean;
  title: string;
}) {
  return (
    <Dialog closeLabel={closeLabel} onClose={onClose} opened={opened} size="sm" title={title}>
      {children}
    </Dialog>
  );
}
