import type { ReactNode } from 'react';
import { ActionButton } from './ActionButton';
import { Dialog } from './Dialog';
import { FormActions } from './FormActions';

interface ConfirmationDialogProps {
  opened: boolean;
  title: string;
  children: ReactNode;
  busy: boolean;
  disabled: boolean;
  cancelLabel: string;
  onCancel: () => void;
  onConfirm: () => void;
}

export function ConfirmationDialog(props: ConfirmationDialogProps) {
  return (
    <Dialog
      opened={props.opened}
      onClose={props.onCancel}
      title={props.title}
      size="xs"
      busy={props.busy}
      withCloseButton={false}
    >
      {props.children}
      <FormActions>
        <ActionButton data-autofocus disabled={props.busy} onClick={props.onCancel}>
          {props.cancelLabel}
        </ActionButton>
        <ActionButton
          tone="critical"
          emphasis="strong"
          disabled={props.disabled}
          loading={props.busy}
          onClick={props.onConfirm}
        >
          {props.title}
        </ActionButton>
      </FormActions>
    </Dialog>
  );
}
