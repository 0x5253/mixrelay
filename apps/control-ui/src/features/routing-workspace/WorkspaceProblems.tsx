import { useEffect, useEffectEvent, useMemo } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import type { GraphApplicationQueue } from '../../core/application/workspace/GraphApplicationQueue';
import type { AudioGraphModel } from '../../core/audio-graph/model';
import { ActionButton } from '../../core/ui/ActionButton';
import { InlineNotice } from '../../core/ui/InlineNotice';
import { RecoveryActions } from '../../core/ui/RecoveryActions';
import { RetryAlert } from '../../core/ui/RetryAlert';
import { Stack } from '../../core/ui/Stack';
import { ProcessorParameterAlert } from './ProcessorParameterAlert';
import type { useGraphLayout } from './useGraphLayout';
import type { useProcessorParameterPublisher } from './useProcessorParameterPublisher';
import type { useRoutingInventory } from './useRoutingInventory';

export type WorkspaceState = 'active' | 'applying' | 'loading' | 'unavailable' | 'blocked';

/** Current workspace problems as stable codes; critical ones stop edits from reaching audio. */
export interface WorkspaceProblemSummary {
  codes: readonly string[];
  tone: 'warning' | 'critical' | null;
}

export const noWorkspaceProblems: WorkspaceProblemSummary = { codes: [], tone: null };

export function workspaceProblemSummary({
  graphError,
  parameterError,
  inventoryFailed,
  persistenceFailed,
  layoutFailed,
}: {
  graphError: string | null;
  parameterError: string | undefined;
  inventoryFailed: boolean;
  persistenceFailed: boolean;
  layoutFailed: boolean;
}): WorkspaceProblemSummary {
  const critical = [graphError, parameterError, inventoryFailed ? 'inventory_failed' : null].filter(
    (code): code is string => Boolean(code),
  );
  const warning = [
    persistenceFailed ? 'graph_persistence_failed' : null,
    layoutFailed ? 'graph_layout_failed' : null,
  ].filter((code): code is string => Boolean(code));
  if (critical.length) return { codes: [...critical, ...warning], tone: 'critical' };
  return warning.length ? { codes: warning, tone: 'warning' } : noWorkspaceProblems;
}

interface WorkspaceProblemsProps {
  /** Where the diagnostics window shows problems; nothing renders while it is closed. */
  target?: HTMLElement | null;
  onChange?: (summary: WorkspaceProblemSummary) => void;
  errorCode: string | null;
  state: WorkspaceState;
  application: GraphApplicationQueue<AudioGraphModel>;
  parameters: ReturnType<typeof useProcessorParameterPublisher>;
  inventory: ReturnType<typeof useRoutingInventory>;
  persistenceError: string | null;
  layout: ReturnType<typeof useGraphLayout>;
}

/** Workspace failures and their recovery live in the diagnostics window, not over the canvas. */
export function WorkspaceProblems({ target, onChange, ...props }: WorkspaceProblemsProps) {
  const parameterError = props.parameters.failures[0]?.code;
  const summary = useMemo(
    () =>
      workspaceProblemSummary({
        graphError: props.errorCode,
        parameterError,
        inventoryFailed: props.inventory.failed,
        persistenceFailed: Boolean(props.persistenceError),
        layoutFailed: Boolean(props.layout.error),
      }),
    [
      props.errorCode,
      parameterError,
      props.inventory.failed,
      props.persistenceError,
      props.layout.error,
    ],
  );
  // The owner hears about changed problems, not about a new callback identity.
  const report = useEffectEvent((next: WorkspaceProblemSummary) => onChange?.(next));
  useEffect(() => report(summary), [summary]);
  useEffect(() => () => report(noWorkspaceProblems), []);
  if (!target || !summary.codes.length) return null;
  return createPortal(
    <Stack>
      <GraphSyncError
        errorCode={props.errorCode}
        state={props.state}
        application={props.application}
      />
      <ProcessorParameterAlert
        parameters={props.parameters}
        applying={props.state === 'applying'}
        onReload={() => props.application.reload()}
      />
      <InventoryError inventory={props.inventory} />
      <GraphPersistenceWarning error={props.persistenceError} />
      <LayoutError layout={props.layout} />
    </Stack>,
    target,
  );
}

function GraphPersistenceWarning({ error }: { error: string | null }) {
  const { t } = useTranslation();
  return error ? (
    <InlineNotice tone="warning">{t('routing.persistenceWarning')}</InlineNotice>
  ) : null;
}

function LayoutError({ layout }: { layout: ReturnType<typeof useGraphLayout> }) {
  const { t } = useTranslation();
  if (!layout.error) return null;
  return (
    <InlineNotice
      tone="warning"
      inert={layout.loading || layout.saving}
      actions={
        <ActionButton onClick={() => void layout.controller.retry()}>
          {t('routing.inventory.retry')}
        </ActionButton>
      }
    >
      {t('routing.layout.failed')}
    </InlineNotice>
  );
}

function InventoryError({ inventory }: { inventory: ReturnType<typeof useRoutingInventory> }) {
  const { t } = useTranslation();
  if (!inventory.failed) return null;
  return (
    <RetryAlert
      message={t('routing.inventory.failed')}
      retryLabel={t('routing.inventory.retry')}
      busy={inventory.refreshing}
      onRetry={inventory.refresh}
    />
  );
}

function GraphSyncError({
  errorCode,
  state,
  application,
}: {
  errorCode: string | null;
  state: WorkspaceState;
  application: GraphApplicationQueue<AudioGraphModel>;
}) {
  const { t } = useTranslation();
  if (!errorCode) return null;
  const blocked = state === 'blocked';
  return (
    <InlineNotice
      tone="critical"
      actions={
        blocked ? (
          errorCode === 'graph_conflict_unresolved' ? (
            <RecoveryActions
              retryLabel={t('routing.recovery.retry')}
              reloadLabel={t('routing.recovery.reload')}
              onRetry={() => application.retry()}
              onReload={() => application.reload()}
            />
          ) : (
            <ActionButton onClick={() => application.retry()}>
              {t('routing.inventory.retry')}
            </ActionButton>
          )
        ) : undefined
      }
    >
      <div>
        {errorCode === 'graph_apply_timeout'
          ? t('routing.recovery.timeout')
          : t(`routing.errors.${errorCode}`, {
              defaultValue: t('routing.errors.control_request_failed'),
            })}
      </div>
      {blocked ? <div>{t('routing.recovery.draftPreserved')}</div> : null}
    </InlineNotice>
  );
}
