import { type ReactNode, useContext, useEffect, useEffectEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { MidiAssignment, MidiBinding } from '../../core/domain/midi';
import { MidiContext, MidiPanelContext } from '../../core/midi/MidiContext';
import type { MidiPort } from '../../core/ports/MidiPort';
import { ActionButton } from '../../core/ui/ActionButton';
import { ContentSection } from '../../core/ui/ContentSection';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { hasPlatformModifier, platformModifier } from '../../core/ui/platformShortcut';
import { Stack } from '../../core/ui/Stack';
import { Text } from '../../core/ui/Text';
import styles from './MidiAccess.module.css';
import { MidiBindingsList } from './MidiBindingsList';
import { useMidiSession } from './useMidiSession';

export function MidiSettingsButton() {
  const midi = useContext(MidiContext);
  const { t } = useTranslation();
  return midi ? (
    <ActionButton
      density="compact"
      data-midi-toggle
      aria-pressed={midi.active}
      aria-label={t('midi.mapMode')}
      aria-keyshortcuts={`${platformModifier()}+M`}
      onClick={midi.toggle}
    >
      MIDI
    </ActionButton>
  ) : null;
}
export function MidiAccess({ port, children }: { port: MidiPort | null; children: ReactNode }) {
  return port ? <ConnectedMidiAccess port={port}>{children}</ConnectedMidiAccess> : children;
}
function ConnectedMidiAccess({ port, children }: { port: MidiPort; children: ReactNode }) {
  const [selected, setSelected] = useState<MidiAssignment | null>(null);
  const session = useMidiSession(port, true);
  const active = session.status?.mapMode ?? false;
  const end = async () => {
    if (await session.command('midi_cancel')) {
      setSelected(null);
      document.querySelector<HTMLElement>('[data-midi-toggle]')?.focus();
    }
  };
  const toggle = async () => {
    if (active) return end();
    if (await session.command('midi_map', { enabled: true })) {
      setSelected(null);
    }
  };
  const assign = async (assignment: MidiAssignment) => {
    if (
      await session.command('midi_learn', {
        assignment: {
          target: assignment.target,
          action: assignment.action,
          graphName: assignment.graphName,
          nodeName: assignment.nodeName,
        },
      })
    )
      setSelected(assignment);
  };
  const remove = async (binding: MidiBinding) => {
    if (await session.command('midi_remove', { binding })) {
      setSelected(null);
      document.querySelector<HTMLElement>('[data-midi-toggle]')?.focus();
    }
  };
  useMidiKeys(
    port,
    () => void toggle(),
    () => void end(),
    active,
  );
  return (
    <MidiContext.Provider
      value={{
        active,
        status: session.status,
        selected,
        toggle: () => void toggle(),
        assign: (assignment) => void assign(assignment),
        remove: (binding) => void remove(binding),
      }}
    >
      <MidiPanelContext.Provider
        value={
          active && session.status ? (
            <MidiPanel
              session={session}
              status={session.status}
              selected={selected}
              onSelect={(assignment) => void assign(assignment)}
              onChange={async (identity, binding) => {
                const saved = await session.command('midi_save', { identity, binding });
                if (saved) setSelected({ ...binding, label: binding.nodeName });
                return saved;
              }}
              onRemove={(binding) => void remove(binding)}
              onClose={() => void end()}
            />
          ) : null
        }
      >
        <div className={styles.workspace} data-midi-mode={active || undefined}>
          {children}
          {session.error && !active ? <MidiError /> : null}
        </div>
      </MidiPanelContext.Provider>
    </MidiContext.Provider>
  );
}
function MidiPanel({
  session,
  status,
  selected,
  onSelect,
  onChange,
  onRemove,
  onClose,
}: {
  session: ReturnType<typeof useMidiSession>;
  status: import('../../core/domain/midi').MidiStatus;
  selected: MidiAssignment | null;
  onSelect: (assignment: MidiAssignment) => void;
  onChange: (identity: MidiBinding, binding: MidiBinding) => Promise<boolean>;
  onRemove: (binding: MidiBinding) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  return (
    <aside data-midi-panel className={styles.panel} aria-label={t('midi.title')}>
      <ContentSection title={t('midi.title')}>
        <Stack>
          <Text>
            {t(selected ? 'midi.waitingTarget' : 'midi.selectTarget', {
              name: selected?.label,
              action: selected ? t(`midi.actions.${selected.action}`) : '',
            })}
          </Text>
          {session.error ? <InlineNotice tone="critical">{t('midi.failed')}</InlineNotice> : null}
          <MidiBindingsList
            status={status}
            busy={session.busy}
            selected={selected}
            onSelect={onSelect}
            onChange={onChange}
            onRemove={onRemove}
          />
          <ActionButton onClick={onClose}>{t('midi.done')}</ActionButton>
        </Stack>
      </ContentSection>
    </aside>
  );
}

function MidiError() {
  const { t } = useTranslation();
  return <InlineNotice tone="critical">{t('midi.failed')}</InlineNotice>;
}

function useMidiKeys(port: MidiPort, toggle: () => void, end: () => void, active: boolean) {
  const handle = useEffectEvent((event: KeyboardEvent) => {
    if (event.repeat || event.isComposing) return;
    if (
      event.key.toLowerCase() === 'm' &&
      hasPlatformModifier(event) &&
      !event.altKey &&
      !event.shiftKey
    ) {
      event.preventDefault();
      toggle();
    } else if (event.key === 'Escape' && active && !event.defaultPrevented) {
      event.preventDefault();
      end();
    }
  });
  const nativeToggle = useEffectEvent(toggle);
  useEffect(() => port.subscribeMapToggle?.(nativeToggle), [port]);
  useEffect(() => {
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, []);
}
