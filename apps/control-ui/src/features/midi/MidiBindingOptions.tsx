import { useTranslation } from 'react-i18next';
import { type MidiBinding, midiAction, midiMode } from '../../core/domain/midi';
import { SelectField } from '../../core/ui/SelectField';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';

export function MidiBindingOptions({
  binding,
  busy,
  onChange,
}: {
  binding: MidiBinding;
  busy: boolean;
  onChange: (binding: MidiBinding) => void;
}) {
  const { t } = useTranslation();
  const ranges = Array.from({ length: 145 }, (_, index) => {
    const value = -60 + index * 0.5;
    return { value: String(value), label: `${value} dB` };
  });
  if (['record', 'pause', 'stop'].includes(binding.action))
    return (
      <SelectField
        label={t('midi.recorderAction')}
        value={binding.action}
        disabled={busy}
        data={['record', 'pause', 'stop'].map((value) => ({
          value,
          label: t(`midi.actions.${value}`),
        }))}
        onChange={(value) => onChange({ ...binding, action: midiAction(value) })}
      />
    );
  if (binding.action !== 'volume') return null;
  return (
    <Stack>
      <SelectField
        label={t('midi.mode')}
        value={binding.mode}
        disabled={busy}
        data={['absolute', 'twos_complement', 'offset64'].map((value) => ({
          value,
          label: t(`midi.modes.${value}`),
        }))}
        onChange={(value) => onChange({ ...binding, mode: midiMode(value) })}
      />
      {binding.mode === 'absolute' ? (
        <>
          <Text kind="supporting">{t('midi.pickup')}</Text>
          <SelectField
            label={t('midi.min')}
            value={String(binding.minDb)}
            data={ranges}
            disabled={busy}
            onChange={(value) => onChange({ ...binding, minDb: Number(value) })}
          />
          <SelectField
            label={t('midi.max')}
            value={String(binding.maxDb)}
            data={ranges}
            disabled={busy}
            onChange={(value) => onChange({ ...binding, maxDb: Number(value) })}
          />
        </>
      ) : (
        <SelectField
          label={t('midi.step')}
          value={String(binding.stepDb)}
          disabled={busy}
          data={[0.5, 1, 2, 3].map((value) => ({ value: String(value), label: `${value} dB` }))}
          onChange={(value) => onChange({ ...binding, stepDb: Number(value) })}
        />
      )}
    </Stack>
  );
}
