import { type FormEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';
import type { ChannelConnectionPlan } from '../domain/graph/channelConnections';
import type { GraphConnection } from '../domain/graph/topology';
import { ActionButton } from '../ui/ActionButton';
import { Dialog } from '../ui/Dialog';
import { FormActions } from '../ui/FormActions';
import { InlineNotice } from '../ui/InlineNotice';
import { SelectField } from '../ui/SelectField';
import { Stack } from '../ui/Stack';
import { Text } from '../ui/Text';
import styles from './ChannelConnectionDialog.module.css';
import {
  acceptedConnections,
  availableChannels,
  type ChannelRange,
  channelConnectionForm,
  initialChoices,
  type NamedPort,
  rangeFromSelection,
  rangeStartingAt,
} from './channelConnectionForm';
import type { AudioGraphModel, AudioGraphNode } from './model';
import { namedAudioPorts } from './portNames';

export interface ChannelConnectionDialogProps {
  model: AudioGraphModel;
  sourceNodeId: string;
  /** Output ports chosen on the canvas; kept as a sparse set until a range is chosen. */
  sourcePortIds?: readonly string[];
  onCancel: () => void;
  /** Receives the accepted connections, validated again against `model` on submit. */
  onConnect: (connections: GraphConnection[]) => void;
}

/** Keyboard equivalent of dragging cables: the same mapping as a pointer drag, with
 * each source channel's destination and every skipped channel stated before commit. */
export function ChannelConnectionDialog({
  model,
  sourceNodeId,
  sourcePortIds,
  onCancel,
  onConnect,
}: ChannelConnectionDialogProps) {
  const { t } = useTranslation();
  const [choices, setChoices] = useState(() => initialChoices(model, sourceNodeId, sourcePortIds));
  const form = channelConnectionForm(model, sourceNodeId, sourcePortIds, choices);
  const { outputs, selection, sourceRange, targets, target, inputs, plan } = form;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    const accepted = acceptedConnections(model, form.request);
    if (accepted.length) onConnect(accepted);
  };
  return (
    <Dialog opened onClose={onCancel} title={t('routing.connect.title')} withCloseButton={false}>
      <form onSubmit={submit}>
        <Stack>
          <Text kind="supporting">{t('routing.connect.source', { node: form.sourceTitle })}</Text>
          {sourceRange ? (
            <SourceRange
              outputs={outputs}
              range={sourceRange}
              onChange={(range) => setChoices((current) => ({ ...current, range }))}
            />
          ) : (
            <Stack gap="tight">
              <Text kind="supporting">
                {t('routing.connect.selected', {
                  channels: selection.map(({ channel }) => `CH ${channel}`).join(', '),
                })}
              </Text>
              <FormActions align="start">
                <ActionButton
                  type="button"
                  onClick={() =>
                    setChoices((current) => ({
                      ...current,
                      range: rangeFromSelection(outputs, selection),
                    }))
                  }
                >
                  {t('routing.connect.useRange')}
                </ActionButton>
              </FormActions>
            </Stack>
          )}
          {targets.length ? (
            <SelectField
              // A kept canvas selection has no source fields, so the destination comes first.
              autoFocus={!sourceRange}
              label={t('routing.connect.target')}
              placeholder={t('routing.connect.chooseTarget')}
              value={target ? target.id : null}
              data={targets}
              onChange={(targetNodeId) =>
                setChoices((current) => ({ ...current, targetNodeId, targetStartPortId: null }))
              }
            />
          ) : (
            <InlineNotice>{t('routing.connect.noTargets')}</InlineNotice>
          )}
          {target ? (
            <SelectField
              label={t('routing.connect.targetStart')}
              value={form.targetAnchorPortId}
              placeholder={t('routing.connect.chooseChannel')}
              data={inputs.map(portOption)}
              onChange={(targetStartPortId) =>
                setChoices((current) => ({ ...current, targetStartPortId }))
              }
            />
          ) : null}
          <ConnectionPreview
            plan={plan}
            model={model}
            sourceNodeId={sourceNodeId}
            pending={!form.request}
          />
        </Stack>
        <FormActions>
          <ActionButton type="button" onClick={onCancel}>
            {t('common.cancel')}
          </ActionButton>
          <ActionButton type="submit" emphasis="strong" disabled={!plan.accepted.length}>
            {t('routing.connect.submit')}
          </ActionButton>
        </FormActions>
      </form>
    </Dialog>
  );
}

function portOption({ port, name, channel }: NamedPort) {
  return { value: port.id, label: `${name}, CH ${channel}` };
}

function SourceRange({
  outputs,
  range,
  onChange,
}: {
  outputs: readonly NamedPort[];
  range: ChannelRange;
  onChange: (range: ChannelRange) => void;
}) {
  const { t } = useTranslation();
  // A first channel that disappeared stays unselected until the user picks another.
  const available = availableChannels(outputs, range);
  return (
    <>
      <SelectField
        autoFocus
        label={t('routing.connect.sourceStart')}
        value={available ? range.startPortId : null}
        placeholder={t('routing.connect.chooseChannel')}
        data={outputs.map(portOption)}
        onChange={(startPortId) => onChange(rangeStartingAt(outputs, range, startPortId))}
      />
      <SelectField
        label={t('routing.connect.count')}
        disabled={available === 0}
        value={available ? String(Math.min(range.count, available)) : null}
        data={Array.from({ length: available }, (_, index) => ({
          value: String(index + 1),
          label: String(index + 1),
        }))}
        onChange={(count) => onChange({ ...range, count: Number(count) })}
      />
    </>
  );
}

function channelOf(
  model: AudioGraphModel,
  nodeId: string,
  portId: string,
  side: 'input' | 'output',
) {
  // The preview only lists ports of nodes the current plan was built from.
  const node = model.nodes.find((candidate) => candidate.id === nodeId) as AudioGraphNode;
  const ports = namedAudioPorts(side === 'input' ? node.inputPorts : node.outputPorts);
  return `CH ${(ports.find(({ port }) => port.id === portId) as NamedPort).channel}`;
}

function ConnectionPreview({
  plan,
  model,
  sourceNodeId,
  pending,
}: {
  plan: ChannelConnectionPlan;
  model: AudioGraphModel;
  sourceNodeId: string;
  pending: boolean;
}) {
  const { t } = useTranslation();
  const sourceChannel = (portId: string) => channelOf(model, sourceNodeId, portId, 'output');
  return (
    <Stack gap="tight">
      <Text kind="label" as="span">
        {t('routing.connect.preview')}
      </Text>
      {/* One polite summary; the list below stays readable without re-announcing every row. */}
      <Text kind="supporting" tone="muted" as="span">
        <span role="status" aria-live="polite">
          {pending
            ? t('routing.connect.pending')
            : t('routing.connect.summary', {
                accepted: plan.accepted.length,
                skipped: plan.skipped.length,
              })}
        </span>
      </Text>
      {pending ? null : (
        <ul className={styles.preview} aria-label={t('routing.connect.preview')}>
          {plan.accepted.map((connection) => (
            <li key={connection.sourcePortId}>
              {t('routing.connect.pair', {
                source: sourceChannel(connection.sourcePortId),
                target: channelOf(model, connection.targetNodeId, connection.targetPortId, 'input'),
              })}
            </li>
          ))}
          {plan.skipped.map((skip) => (
            <li key={skip.sourcePortId} data-skipped="">
              {t('routing.connect.skipped', {
                source: sourceChannel(skip.sourcePortId),
                reason: t(`routing.connect.reasons.${skip.reason}`),
              })}
            </li>
          ))}
        </ul>
      )}
    </Stack>
  );
}
