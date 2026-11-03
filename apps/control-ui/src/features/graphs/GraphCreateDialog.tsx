import { useState } from 'react';
import { useTranslation } from 'react-i18next';

import { ActionButton } from '../../core/ui/ActionButton';
import { Dialog } from '../../core/ui/Dialog';
import { FormActions } from '../../core/ui/FormActions';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { Stack } from '../../core/ui/Stack';
import { TextField } from '../../core/ui/TextField';

export interface GraphCreateDialogProps {
  busy?: boolean;
  creating?: boolean;
  error?: string | null;
  onCancel: () => void;
  onCreate: (name: string) => void;
  suggestedName: string;
}

export function GraphCreateDialog({
  onCancel,
  onCreate,
  suggestedName,
  busy,
  creating = false,
  error,
}: GraphCreateDialogProps) {
  const { t } = useTranslation();
  const [name, setName] = useState(suggestedName);
  const normalizedName = name.trim();

  return (
    <Dialog
      opened
      title={t('studio.newGraph')}
      onClose={onCancel}
      busy={creating}
      withCloseButton={false}
      size="xs"
    >
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (normalizedName && !busy) onCreate(normalizedName);
        }}
      >
        <Stack>
          <TextField
            data-autofocus
            label={t('studio.graphName')}
            disabled={busy}
            onChange={(event) => setName(event.currentTarget.value)}
            value={name}
          />
          {error ? <InlineNotice tone="critical">{error}</InlineNotice> : null}
        </Stack>

        <FormActions>
          <ActionButton disabled={creating} onClick={onCancel} type="button">
            {t('common.cancel')}
          </ActionButton>
          <ActionButton disabled={!normalizedName || busy} type="submit" emphasis="strong">
            {t('common.create')}
          </ActionButton>
        </FormActions>
      </form>
    </Dialog>
  );
}
