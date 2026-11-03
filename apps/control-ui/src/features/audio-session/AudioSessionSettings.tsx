import { useLayoutEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { ControlPortError } from '../../core/domain/control/errors';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { AudioSessionStatus } from '../../core/domain/control/status';
import type { GraphPort } from '../../core/ports/control/GraphPort';
import { ActionButton } from '../../core/ui/ActionButton';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { InlineSelect } from '../../core/ui/InlineSelect';
import styles from './AudioSessionControl.module.css';
import { waitForGraphActivation } from './application/waitForGraphActivation';

/** The engine capabilities this module uses. */
type SessionPort = Pick<GraphPort, 'getGraph' | 'getGraphStatus' | 'reconfigureSession'>;

export interface AudioSessionSettingsProps {
  disabled?: boolean;
  controlPort: SessionPort;
  session: AudioSessionStatus;
  onApplied: (document: RuntimeGraphDocument) => void;
  onBusyChange: (busy: boolean) => void;
  sessionAlerts?: readonly string[];
}

function useSessionSettings({
  controlPort,
  session,
  onApplied,
  onBusyChange,
  disabled = false,
}: AudioSessionSettingsProps) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const [selection, setSelection] = useState<{
    port: SessionPort;
    base: AudioSessionStatus['format'];
    frames: string;
  } | null>(null);
  const frames =
    selection?.port === controlPort &&
    (busy ||
      (selection.base.sampleRate === session.format.sampleRate &&
        selection.base.blockFrames === session.format.blockFrames))
      ? selection.frames
      : String(session.format.blockFrames);
  const [error, setError] = useState<{ message: string; frames: string } | null>(null);
  const generation = useRef(0);
  const ownerPort = useRef(controlPort);
  const releaseBusy = useRef<(() => void) | null>(null);
  useLayoutEffect(() => {
    ownerPort.current = controlPort;
    generation.current += 1;
    setBusy(false);
    setError(null);
    setSelection(null);
    return () => {
      // Native requests cannot be cancelled, but their replies must never reach
      // a replacement port/owner. Release this owner's busy lease exactly once.
      generation.current += 1;
      const release = releaseBusy.current;
      releaseBusy.current = null;
      release?.();
    };
  }, [controlPort]);
  const apply = async (nextFrames: string) => {
    if (
      disabled ||
      !session.capabilities.canReconfigureLive ||
      releaseBusy.current ||
      !controlPort.reconfigureSession ||
      !Number.isFinite(session.format.sampleRate) ||
      session.format.sampleRate <= 0 ||
      !session.capabilities.supportedBlockFrames.includes(Number(nextFrames))
    )
      return;
    const requestGeneration = generation.current;
    const isCurrent = () =>
      generation.current === requestGeneration && ownerPort.current === controlPort;
    releaseBusy.current = () => onBusyChange(false);
    setBusy(true);
    setError(null);
    setSelection({ port: controlPort, base: session.format, frames: nextFrames });
    onBusyChange(true);
    try {
      const next = await reconfigure(
        controlPort,
        controlPort.reconfigureSession.bind(controlPort),
        {
          sampleRate: session.format.sampleRate,
          blockFrames: Number(nextFrames),
        },
        isCurrent,
      );
      if (next && isCurrent()) onApplied(next);
    } catch (cause) {
      if (isCurrent()) {
        setSelection(null);
        setError({
          message: t(sessionErrorKey(cause), { defaultValue: t('audioSession.changeFailed') }),
          frames: nextFrames,
        });
      }
    } finally {
      if (isCurrent()) {
        const release = releaseBusy.current;
        releaseBusy.current = null;
        release?.();
        setBusy(false);
      }
    }
  };
  return { frames, busy, error, apply };
}

export function AudioSessionSettings(props: AudioSessionSettingsProps) {
  const { t } = useTranslation();
  const { session, controlPort, disabled, sessionAlerts = [] } = props;
  const { frames, busy, error, apply } = useSessionSettings(props);
  const retryFrames = error?.frames ?? frames;
  return (
    <>
      <InlineSelect
        label={t('audioSession.processingBuffer')}
        value={frames}
        title={t(
          session.capabilities.canReconfigureLive
            ? 'audioSession.changeNotice'
            : 'audioSession.locked',
        )}
        onChange={(value) => void apply(value)}
        disabled={
          busy ||
          disabled ||
          !controlPort.reconfigureSession ||
          !session.capabilities.supportedBlockFrames.length
        }
        data={[
          ...new Set([session.format.blockFrames, ...session.capabilities.supportedBlockFrames]),
        ]
          .sort((a, b) => a - b)
          .map((value) => ({
            value: String(value),
            label: String(value),
          }))}
      />
      <AudioSessionAlerts messages={[error?.message ?? '', ...sessionAlerts]} />
      {session.issue || error ? (
        <ActionButton
          density="compact"
          disabled={
            busy ||
            disabled ||
            !controlPort.reconfigureSession ||
            !session.capabilities.supportedBlockFrames.includes(Number(retryFrames))
          }
          onClick={() => void apply(retryFrames)}
        >
          {t('audioSession.apply')}
        </ActionButton>
      ) : null}
    </>
  );
}

/** A single stacked surface prevents native and request failures from covering each other. */
export function AudioSessionAlerts({ messages }: { messages: readonly string[] }) {
  const visible = [...new Set(messages.filter(Boolean))];
  return visible.length ? (
    <div className={styles.error}>
      <InlineNotice tone="critical">
        {visible.map((message) => (
          <p key={message}>{message}</p>
        ))}
      </InlineNotice>
    </div>
  ) : null;
}

function sessionErrorKey(cause: unknown): string {
  return cause instanceof ControlPortError
    ? `audioSession.errors.${cause.body.code}`
    : 'audioSession.changeFailed';
}

async function reconfigure(
  controlPort: SessionPort,
  reconfigureSession: NonNullable<GraphPort['reconfigureSession']>,
  format: AudioSessionStatus['format'],
  isCurrent: () => boolean,
): Promise<RuntimeGraphDocument | null> {
  const graph = await controlPort.getGraph();
  if (!isCurrent()) return null;
  const response = await reconfigureSession({
    expectedRevision: graph.revision,
    format,
  });
  if (!isCurrent()) return null;
  if (
    !(await waitForGraphActivation(
      controlPort,
      response.revision,
      isCurrent,
      timerScheduler,
      monotonicClock,
    ))
  )
    return null;
  if (response.revision === graph.revision) return null;
  return controlPort.getGraph();
}
