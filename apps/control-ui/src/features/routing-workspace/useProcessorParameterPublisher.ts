import { useCallback, useEffect, useRef, useState } from 'react';

import {
  type ParameterPort,
  type PendingProcessorParameter,
  type ProcessorParameterFailure,
  ProcessorParameterQueue,
} from './ProcessorParameterQueue';

/** Adapts the per-port publication owner to mounted UI state. */
export function useProcessorParameterPublisher(
  controlPort: ParameterPort,
  onPersistenceWarning?: (error: string | null) => void,
) {
  const queue = useRef<ProcessorParameterQueue | null>(null);
  const warning = useRef(onPersistenceWarning);
  warning.current = onPersistenceWarning;
  const [busy, setBusy] = useState(false);
  const [failures, setFailures] = useState<readonly ProcessorParameterFailure[]>([]);
  useEffect(() => {
    const session = new ProcessorParameterQueue(controlPort, {
      change: (pending, rejected) => {
        setBusy(pending);
        setFailures(rejected);
      },
      persistenceWarning: (error) => warning.current?.(error),
    });
    queue.current = session;
    setBusy(false);
    setFailures([]);
    return () => {
      session.close();
      queue.current = null;
    };
  }, [controlPort]);

  const publish = useCallback((update: PendingProcessorParameter) => {
    queue.current?.publish(update);
  }, []);
  const retryFailed = useCallback(() => queue.current?.retryFailed(), []);
  const discardFailures = useCallback(() => queue.current?.discardFailures(), []);
  const whenIdle = useCallback(() => queue.current?.whenIdle() ?? Promise.resolve(), []);
  return { busy, publish, whenIdle, failures, retryFailed, discardFailures };
}
