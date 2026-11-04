import { type ReactNode, useContext, useEffect, useEffectEvent, useMemo, useState } from 'react';
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
import { MidiMappingController, midiShortcut } from './application/MidiMappingController';
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
  const mapping = useMemo(
    () =>
      new MidiMappingController(session.command, {
        select: setSelected,
        release: () => document.querySelector<HTMLElement>('[data-midi-toggle]')?.focus(),
      }),
    [session.command],
  );
  const toggle = () => void mapping.toggle(active);
  const end = () => void mapping.end();
  const assign = (assignment: MidiAssignment) => void mapping.assign(assignment);
  const remove = (binding: MidiBinding) => void mapping.remove(binding);
  useMidiKeys(port, toggle, end, active);
  return (
    <MidiContext.Provider
      value={{
        active,
        status: session.status,
        selected,
        toggle,
        assign,
        remove,
      }}
    >
      <MidiPanelContext.Provider
        value={
          active && session.status ? (
            <MidiPanel
              session={session}
              status={session.status}
              selected={selected}
              onSelect={assign}
              onChange={(identity, binding) => mapping.save(identity, binding)}
              onRemove={remove}
              onClose={end}
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
    const command = midiShortcut(event, hasPlatformModifier(event), active);
    if (!command) return;
    event.preventDefault();
    if (command === 'toggle') toggle();
    else end();
  });
  const nativeToggle = useEffectEvent(toggle);
  useEffect(() => port.subscribeMapToggle?.(nativeToggle), [port]);
  useEffect(() => {
    window.addEventListener('keydown', handle);
    return () => window.removeEventListener('keydown', handle);
  }, []);
}
