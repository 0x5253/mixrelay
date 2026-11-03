import { useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { monotonicClock, timerScheduler } from '../../core/adapters/browser/timers';
import { getEnginePresentation } from '../../core/audio-session/presentation';
import type { RuntimeGraphDocument } from '../../core/domain/control/graph';
import type { MixRelayStatus } from '../../core/domain/control/status';
import type { GraphPort } from '../../core/ports/control/GraphPort';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { TransportButton } from '../../core/ui/TransportButton';
import styles from './AudioPauseControl.module.css';
import { waitForEnginePower } from './application/waitForEnginePower';
import { waitForGraphActivation } from './application/waitForGraphActivation';

/** The engine capabilities this module uses. */
type PausePort = Pick<GraphPort, 'getGraph' | 'getGraphStatus' | 'getStatus' | 'setEnginePower'>;

export function AudioPauseControl({
  controlPort,
  status,
  disabled,
  onBusyChange,
  onApplied,
}: {
  controlPort: PausePort;
  status: MixRelayStatus | null;
  disabled?: boolean;
  onBusyChange: (busy: boolean) => void;
  onApplied: (document: RuntimeGraphDocument) => void;
}) {
  const { t } = useTranslation();
  const lock = useRef(false);
  const generation = useRef(0);
  const owner = useRef(controlPort);
  owner.current = controlPort;
  useEffect(
    () => () => {
      generation.current += 1;
    },
    [],
  );
  const [confirmed, setConfirmed] = useState<{
    base: MixRelayStatus | null;
    value: MixRelayStatus;
  }>();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState(false);
  const effectiveStatus = confirmed?.base === status ? confirmed.value : status;
  const power = effectiveStatus?.enginePower;
  const pending = busy || power?.transitioning;
  const health = getEnginePresentation(effectiveStatus, t);
  const action = power?.enabled === false ? 'audioSession.resume' : 'audioSession.pause';
  const label = pending ? t('audioSession.changing') : t(action);
  const accessibleLabel = power ? label : health.label;
  const toggle = async () => {
    if (lock.current || !power || !controlPort.setEnginePower) return;
    lock.current = true;
    setBusy(true);
    setError(false);
    onBusyChange(true);
    const ticket = generation.current;
    const current = () => ticket === generation.current && owner.current === controlPort;
    try {
      const response = await controlPort.setEnginePower(!power.enabled);
      if (
        !(await waitForGraphActivation(
          controlPort,
          response.revision,
          current,
          timerScheduler,
          monotonicClock,
        ))
      )
        return;
      const document = await controlPort.getGraph();
      const next = await waitForEnginePower(
        controlPort,
        !power.enabled,
        current,
        timerScheduler,
        monotonicClock,
      );
      if (!next) return;
      setConfirmed({ base: status, value: next });
      onApplied(document);
    } catch {
      if (current()) setError(true);
    } finally {
      lock.current = false;
      setBusy(false);
      onBusyChange(false);
    }
  };
  return (
    <div className={styles.control}>
      <PauseNotice
        failed={error}
        unsaved={Boolean(power?.unsaved)}
        pausedAfterFailures={power?.pauseReason === 'repeatedFailures'}
      />
      <TransportButton
        icon={power?.enabled === false ? 'resume' : 'pause'}
        label={accessibleLabel}
        description={health.label}
        tooltip={accessibleLabel === health.label ? accessibleLabel : `${label} · ${health.label}`}
        // Pause and resume share one neutral colour; only a failed session is coloured.
        tone={!pending && health.tone === 'critical' ? 'critical' : 'neutral'}
        pressed={power?.enabled === false}
        busy={Boolean(pending)}
        compact
        disabled={Boolean(disabled || pending || !power || !controlPort.setEnginePower)}
        onClick={() => void toggle()}
      />
    </div>
  );
}

/** A failed change is an alert. A pause the engine chose after repeated failures, and an
 * applied change the service has not yet saved for its next start, are warnings read from its
 * status, so they end with Resume or with the service's successful retry. */
function PauseNotice({
  failed,
  unsaved,
  pausedAfterFailures,
}: {
  failed: boolean;
  unsaved: boolean;
  pausedAfterFailures: boolean;
}) {
  const { t } = useTranslation();
  const message = failed
    ? 'audioSession.pauseFailed'
    : pausedAfterFailures
      ? 'audioSession.pausedAfterFailures'
      : unsaved
        ? 'audioSession.pauseNotSaved'
        : undefined;
  if (!message) return null;
  return (
    <div className={styles.error}>
      <InlineNotice tone={failed ? 'critical' : 'warning'}>{t(message)}</InlineNotice>
    </div>
  );
}
