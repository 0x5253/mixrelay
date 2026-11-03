import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { ActionButton } from './ActionButton';
import { Dialog } from './Dialog';
import { FormActions } from './FormActions';
import { InlineNotice } from './InlineNotice';
import { Stack } from './Stack';
import { TextField } from './TextField';

export function NameDialog({
  title,
  initialName,
  allowEmpty = false,
  maxLength = 40,
  description,
  placeholder,
  nameLabel,
  createLabel,
  enabled,
  onCancel,
  onCreate,
  error,
}: {
  title: string;
  initialName: string;
  allowEmpty?: boolean;
  maxLength?: number;
  description?: string;
  placeholder?: string;
  nameLabel: string;
  createLabel: string;
  enabled: boolean;
  onCancel: () => void;
  onCreate: (name: string) => void;
  error?: string | null;
}) {
  const { t } = useTranslation();
  const [name, setName] = useState(initialName);
  return (
    <Dialog opened onClose={onCancel} title={title} size="xs" withCloseButton={false}>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          if (enabled && (allowEmpty || name.trim())) onCreate(name.trim());
        }}
      >
        <Stack>
          <TextField
            data-autofocus
            label={nameLabel}
            value={name}
            maxLength={maxLength}
            description={description}
            placeholder={placeholder}
            disabled={!enabled}
            onChange={(event) => setName(event.currentTarget.value)}
          />
          {error ? <InlineNotice tone="critical">{error}</InlineNotice> : null}
        </Stack>
        <FormActions>
          <ActionButton type="button" onClick={onCancel}>
            {t('common.cancel')}
          </ActionButton>
          <ActionButton
            type="submit"
            emphasis="strong"
            disabled={!enabled || (!allowEmpty && !name.trim())}
          >
            {createLabel}
          </ActionButton>
        </FormActions>
      </form>
    </Dialog>
  );
}
