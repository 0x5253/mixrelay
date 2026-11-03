import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ProcessorRetryRequest } from '../../core/domain/control/graph';
import type { FailedProcessorInstance } from '../../core/domain/control/status';

import { ProcessorRecoveryButton } from './ProcessorRecoveryButton';

type RetryTarget = Omit<ProcessorRetryRequest, 'expectedRevision'>;

export function ProcessorRecoveryControl(props: {
  busy: boolean;
  catalogRevision?: number;
  enabled: boolean;
  failedProcessors?: FailedProcessorInstance[];
  graphId?: string;
  isProcessor: boolean;
  nodeId: string;
  nodeName: string;
  onRetry: (target: RetryTarget) => Promise<unknown>;
  supported: boolean;
}) {
  const { t } = useTranslation();
  const [retryingInstance, setRetryingInstance] = useState<string>();
  const catalogRevision = props.catalogRevision;
  const failed = props.failedProcessors?.find(
    (candidate) => candidate.graphId === props.graphId && candidate.nodeId === props.nodeId,
  );
  if (
    !props.isProcessor ||
    !props.graphId ||
    catalogRevision === undefined ||
    !props.supported ||
    !failed
  )
    return null;

  const retrying = retryingInstance === failed.instanceId;
  const retry = async () => {
    setRetryingInstance(failed.instanceId);
    try {
      await props.onRetry({
        expectedCatalogRevision: catalogRevision,
        expectedInstanceId: failed.instanceId,
        graphId: failed.graphId,
        nodeId: failed.nodeId,
      });
    } catch {
      // The graph writer owns error presentation; this view only tracks its pending lifetime.
    } finally {
      setRetryingInstance((current) => (current === failed.instanceId ? undefined : current));
    }
  };
  return (
    <ProcessorRecoveryButton
      disabled={props.busy || !props.enabled || retrying}
      label={t('routing.processorRecovery.retryFailed', { name: props.nodeName })}
      retrying={retrying}
      onRetry={() => void retry()}
    />
  );
}
