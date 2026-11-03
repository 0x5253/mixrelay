import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  bindingKey,
  bindingLabel,
  bindingTarget,
  controlKey,
  disconnected,
  type MidiAssignment,
  type MidiBinding,
  type MidiControl,
  type MidiStatus,
  sameTarget,
} from '../../core/domain/midi';
import { ActionButton } from '../../core/ui/ActionButton';
import { ContentSection } from '../../core/ui/ContentSection';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';
import { MidiBindingOptions } from './MidiBindingOptions';

export function MidiBindingsList({
  status,
  busy,
  selected,
  onSelect,
  onChange,
  onRemove,
}: {
  status: MidiStatus;
  busy: boolean;
  selected: MidiAssignment | null;
  onSelect: (assignment: MidiAssignment) => void;
  onChange: (identity: MidiBinding, binding: MidiBinding) => Promise<boolean> | undefined;
  onRemove: (binding: MidiBinding) => void;
}) {
  const { t } = useTranslation();
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  return (
    <Stack gap="section">
      {status.unavailable ? (
        <InlineNotice tone="warning" title={t('midi.unavailable')}>
          {status.unavailable}
        </InlineNotice>
      ) : null}
      {!status.bindings.length ? <Text>{t('midi.empty')}</Text> : null}
      {Array.from(groupBindings(status.bindings), ([key, { control, bindings }]) => (
        <ContentSection key={key} title={controlLabel(control, t)} level={3} divider="before">
          {bindings.map((binding) => (
            <BindingRow
              key={bindingKey(binding)}
              binding={binding}
              status={status}
              busy={busy}
              chosen={
                selectedKey === bindingKey(binding) &&
                Boolean(
                  selected &&
                    sameTarget(selected.target, binding.target) &&
                    selected.action === binding.action,
                )
              }
              onSelect={() => {
                setSelectedKey(bindingKey(binding));
                onSelect({ ...binding, label: bindingLabel(binding) });
              }}
              onChange={async (next) => {
                if (await onChange(binding, next)) setSelectedKey(bindingKey(next));
              }}
              onRemove={() => onRemove(binding)}
            />
          ))}
        </ContentSection>
      ))}
    </Stack>
  );
}

function controlLabel(control: MidiControl, t: import('i18next').TFunction) {
  return t('midi.control', {
    device: control.device.name,
    channel: control.channel + 1,
    kind: t(`midi.kinds.${control.kind}`),
    number: control.number,
  });
}

function BindingRow({
  binding,
  status,
  busy,
  chosen,
  onSelect,
  onChange,
  onRemove,
}: {
  binding: MidiBinding;
  status: MidiStatus;
  busy: boolean;
  chosen: boolean;
  onSelect: () => void;
  onChange: (binding: MidiBinding) => Promise<void>;
  onRemove: () => void;
}) {
  const { t } = useTranslation();
  const label = bindingLabel(binding);
  return (
    <Stack>
      <ActionButton
        align="start"
        stretch
        aria-pressed={chosen}
        autoFocus={chosen}
        disabled={busy}
        aria-label={t('midi.selectMapping', {
          name: label,
          action: t(`midi.actions.${binding.action}`),
        })}
        onClick={onSelect}
        onKeyDown={(event) => {
          if (event.key === 'Delete' || event.key === 'Backspace') {
            event.preventDefault();
            onRemove();
          } else event.continuePropagation();
        }}
      >
        {`${label} · ${t(`midi.actions.${binding.action}`)}`}
      </ActionButton>
      <BindingStatus status={status} binding={binding} />
      {chosen ? <MidiBindingOptions binding={binding} busy={busy} onChange={onChange} /> : null}
      <ActionButton
        disabled={busy}
        aria-label={t('midi.removeTarget', { name: label })}
        onClick={onRemove}
      >
        {t('midi.remove')}
      </ActionButton>
    </Stack>
  );
}

function BindingStatus({ status, binding }: { status: MidiStatus; binding: MidiBinding }) {
  const { t } = useTranslation();
  return (
    <>
      {bindingTarget(status, binding) && !disconnected(status, binding.control) ? (
        <Text kind="supporting">{t('common.ready')}</Text>
      ) : null}
      {!bindingTarget(status, binding) && status.targetsAvailable ? (
        <Text tone="warning">{t('midi.targetMissing')}</Text>
      ) : null}
      {!status.targetsAvailable ? <Text tone="warning">{t('midi.targetUnavailable')}</Text> : null}
      {disconnected(status, binding.control) ? (
        <Text tone="warning">{t('midi.disconnected')}</Text>
      ) : null}
    </>
  );
}

function groupBindings(bindings: MidiBinding[]) {
  const groups = new Map<string, { control: MidiControl; bindings: MidiBinding[] }>();
  for (const binding of bindings) {
    const key = controlKey(binding.control);
    const group = groups.get(key);
    if (group) group.bindings.push(binding);
    else groups.set(key, { control: binding.control, bindings: [binding] });
  }
  return groups;
}
