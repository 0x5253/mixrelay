import { type FormEvent, useState } from 'react';
import { useTranslation } from 'react-i18next';
import {
  type ChannelConnectionPlan,
  type ChannelConnectionRequest,
  planChannelConnections,
} from '../domain/graph/channelConnections';
import { type GraphConnection, graphConnectionRejection } from '../domain/graph/topology';
import { ActionButton } from '../ui/ActionButton';
import { Dialog } from '../ui/Dialog';
import { FormActions } from '../ui/FormActions';
import { InlineNotice } from '../ui/InlineNotice';
import { SelectField } from '../ui/SelectField';
import { Stack } from '../ui/Stack';
import { Text } from '../ui/Text';
import styles from './ChannelConnectionDialog.module.css';
import { type AudioGraphModel, type AudioGraphNode, audioGraphNodeTitle } from './model';
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

interface Range {
  startPortId: string;
  count: number;
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
  const source = model.nodes.find((node) => node.id === sourceNodeId);
  const outputs = source ? namedAudioPorts(source.outputPorts) : [];
  const selection = outputs.filter(({ port }) => sourcePortIds?.includes(port.id));
  const [range, setRange] = useState<Range | null>(() =>
    selection.length ? null : { startPortId: outputs[0]?.port.id ?? '', count: outputs.length },
  );
  const [targetNodeId, setTargetNodeId] = useState<string | null>(null);
  const [targetStartPortId, setTargetStartPortId] = useState<string | null>(null);
  const targets = targetOptions(model, sourceNodeId);
  const target = model.nodes.find((node) => node.id === targetNodeId);
  const inputs = target ? namedAudioPorts(target.inputPorts) : [];
  // Default to the first input only until the user picks one; a picked channel that
  // disappears must be chosen again rather than silently moving the cables.
  const targetAnchor =
    targetStartPortId === null
      ? inputs[0]?.port.id
      : inputs.find(({ port }) => port.id === targetStartPortId)?.port.id;
  // A canvas selection whose channels have all disappeared falls back to choosing a range.
  const sourceRange = range ?? (selection.length ? null : { startPortId: '', count: 1 });
  const request = connectionRequest({
    sourceNodeId,
    ports: sourceRange ? rangePorts(outputs, sourceRange) : selection,
    targetNodeId: target?.id,
    targetAnchorPortId: targetAnchor,
  });
  const plan = request ? planChannelConnections(model, request) : emptyPlan;
  const submit = (event: FormEvent) => {
    event.preventDefault();
    // The graph can change while the dialog is open; commit only what is valid now.
    const accepted = request ? planChannelConnections(model, request).accepted : [];
    if (accepted.length) onConnect(accepted);
  };
  return (
    <Dialog opened onClose={onCancel} title={t('routing.connect.title')} withCloseButton={false}>
      <form onSubmit={submit}>
        <Stack>
          <Text kind="supporting">
            {t('routing.connect.source', {
              node: source ? audioGraphNodeTitle(source) : sourceNodeId,
            })}
          </Text>
          {sourceRange ? (
            <SourceRange outputs={outputs} range={sourceRange} onChange={setRange} />
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
                  onClick={() => setRange(rangeFromSelection(outputs, selection))}
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
              onChange={(value) => {
                setTargetNodeId(value);
                setTargetStartPortId(null);
              }}
            />
          ) : (
            <InlineNotice>{t('routing.connect.noTargets')}</InlineNotice>
          )}
          {target ? (
            <SelectField
              label={t('routing.connect.targetStart')}
              value={targetAnchor ?? null}
              placeholder={t('routing.connect.chooseChannel')}
              data={inputs.map(portOption)}
              onChange={setTargetStartPortId}
            />
          ) : null}
          <ConnectionPreview
            plan={plan}
            model={model}
            sourceNodeId={sourceNodeId}
            pending={!request}
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

type NamedPort = ReturnType<typeof namedAudioPorts>[number];

const emptyPlan: ChannelConnectionPlan = { accepted: [], skipped: [] };

function portOption({ port, name, channel }: NamedPort) {
  return { value: port.id, label: `${name}, CH ${channel}` };
}

/** A node can feed itself only across a host boundary, e.g. a virtual device's
 * system-side output into its own system-side input. */
function acceptsSelfConnection(model: AudioGraphModel, node: AudioGraphNode) {
  return node.outputPorts.some((output) =>
    node.inputPorts.some(
      (input) =>
        graphConnectionRejection(model, {
          sourceNodeId: node.id,
          sourcePortId: output.id,
          targetNodeId: node.id,
          targetPortId: input.id,
        }) !== 'invalid',
    ),
  );
}

/** Nodes with inputs that a connection could reach; repeated titles get an ordinal so
 * each option is distinct. */
function targetOptions(model: AudioGraphModel, sourceNodeId: string) {
  const nodes = model.nodes.filter((node) =>
    node.id === sourceNodeId ? acceptsSelfConnection(model, node) : node.inputPorts.length,
  );
  const titles = nodes.map(audioGraphNodeTitle);
  return nodes.map((node, index) => {
    const title = titles[index] as string;
    const repeats = titles.filter((other) => other === title).length;
    const ordinal = titles.slice(0, index).filter((other) => other === title).length + 1;
    return { value: node.id, label: repeats > 1 ? `${title} (${ordinal})` : title };
  });
}

function rangeFromSelection(outputs: readonly NamedPort[], selection: readonly NamedPort[]): Range {
  const indexes = selection.map((entry) => outputs.indexOf(entry));
  const first = Math.min(...indexes);
  return {
    // Called only with a non-empty selection drawn from `outputs`.
    startPortId: (outputs[first] as NamedPort).port.id,
    count: Math.max(...indexes) - first + 1,
  };
}

/** The outputs a range covers; empty when its first channel no longer exists. */
function rangePorts(outputs: readonly NamedPort[], range: Range) {
  const start = outputs.findIndex(({ port }) => port.id === range.startPortId);
  return start < 0 ? [] : outputs.slice(start, start + range.count);
}

function connectionRequest({
  sourceNodeId,
  ports,
  targetNodeId,
  targetAnchorPortId,
}: {
  sourceNodeId: string;
  ports: readonly NamedPort[];
  targetNodeId: string | undefined;
  targetAnchorPortId: string | undefined;
}): ChannelConnectionRequest | null {
  const anchor = ports[0];
  if (!anchor || !targetNodeId || !targetAnchorPortId) return null;
  return {
    sourceNodeId,
    sourcePortIds: ports.map(({ port }) => port.id),
    sourceAnchorPortId: anchor.port.id,
    targetNodeId,
    targetAnchorPortId,
  };
}

function SourceRange({
  outputs,
  range,
  onChange,
}: {
  outputs: readonly NamedPort[];
  range: Range;
  onChange: (range: Range) => void;
}) {
  const { t } = useTranslation();
  const start = outputs.findIndex(({ port }) => port.id === range.startPortId);
  // A first channel that disappeared stays unselected until the user picks another.
  const available = start < 0 ? 0 : outputs.length - start;
  return (
    <>
      <SelectField
        autoFocus
        label={t('routing.connect.sourceStart')}
        value={start < 0 ? null : range.startPortId}
        placeholder={t('routing.connect.chooseChannel')}
        data={outputs.map(portOption)}
        onChange={(startPortId) => {
          const next = outputs.findIndex(({ port }) => port.id === startPortId);
          onChange({ startPortId, count: Math.min(range.count, outputs.length - next) });
        }}
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
