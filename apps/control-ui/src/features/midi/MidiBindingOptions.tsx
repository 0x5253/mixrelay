import { useTranslation } from 'react-i18next';
import type { MidiBinding } from '../../core/domain/midi';
import { SelectField } from '../../core/ui/SelectField';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';
import { type MidiOptionField, midiBindingOptions } from './domain/midiBindingOptions';

export function MidiBindingOptions({
  binding,
  busy,
  onChange,
}: {
  binding: MidiBinding;
  busy: boolean;
  onChange: (binding: MidiBinding) => void;
}) {
  const options = midiBindingOptions(binding);
  if (!options) return null;
  const select = (field: MidiOptionField) => (
    <OptionSelect field={field} busy={busy} onChange={onChange} />
  );
  if (options.kind === 'recorder') return select(options.action);
  return (
    <Stack>
      {select(options.mode)}
      {options.kind === 'absolute' ? (
        <PickupRange range={options.range} busy={busy} onChange={onChange} />
      ) : (
        select(options.step)
      )}
    </Stack>
  );
}

function PickupRange({
  range: [minimum, maximum],
  busy,
  onChange,
}: {
  range: readonly [MidiOptionField, MidiOptionField];
  busy: boolean;
  onChange: (binding: MidiBinding) => void;
}) {
  const { t } = useTranslation();
  return (
    <>
      <Text kind="supporting">{t('midi.pickup')}</Text>
      <OptionSelect field={minimum} busy={busy} onChange={onChange} />
      <OptionSelect field={maximum} busy={busy} onChange={onChange} />
    </>
  );
}

function OptionSelect({
  field,
  busy,
  onChange,
}: {
  field: MidiOptionField;
  busy: boolean;
  onChange: (binding: MidiBinding) => void;
}) {
  const { t } = useTranslation();
  const choiceLabel = (value: string) => {
    if (field.name === 'recorderAction') return t(`midi.actions.${value}`);
    if (field.name === 'mode') return t(`midi.modes.${value}`);
    return `${value} dB`;
  };
  return (
    <SelectField
      label={t(`midi.${field.name}`)}
      value={field.value}
      disabled={busy}
      data={field.choices.map((value) => ({ value, label: choiceLabel(value) }))}
      onChange={(value) => onChange(field.apply(value))}
    />
  );
}
