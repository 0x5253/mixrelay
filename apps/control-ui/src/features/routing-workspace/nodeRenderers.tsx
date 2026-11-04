import type { TFunction } from 'i18next';
import { useCallback, useLayoutEffect, useMemo, useRef } from 'react';
import {
  type AudioGraphModel,
  type AudioGraphNode,
  audioGraphNodeTitle,
} from '../../core/audio-graph/model';
import type { GraphNodeControls } from '../../core/domain/control/graph';
import { useMidiMappingActive } from '../../core/midi/MidiContext';
import { NodeMuteButton, NodePowerButton } from '../../core/ui/NodeSignalControls';
import { ProcessorEditorAlert } from './ProcessorControlAlerts';
import { ProcessorRecoveryControl } from './ProcessorRecoveryControl';
import { updateRecorderChannels } from './routingNodeGeometry';
import { runtimeNodeId } from './runtimeAudioNode';
import { signalControlVisibility, usesRecoveryControls } from './signalControls';
import { useNodeControls } from './useNodeControls';
import type { useProcessorEditors } from './useProcessorEditors';
import type { WorkspaceViewProps } from './workspaceViewProps';

type NodeControlInputs = Pick<
  WorkspaceViewProps,
  | 'application'
  | 'controlPort'
  | 'documentRevision'
  | 'graphId'
  | 'interactionDisabled'
  | 'layout'
  | 'parameters'
  | 'state'
>;

/** What node controls and recorder content read. Memoized so the canvas render functions
 * keep their identity and nodes re-render only when one of these changes. */
type NodeRenderInputs = Pick<
  WorkspaceViewProps,
  | 'application'
  | 'catalogRevision'
  | 'controlPort'
  | 'documentRevision'
  | 'failedProcessors'
  | 'graphId'
  | 'interactionDisabled'
  | 'playbackPaused'
  | 'renderRecorder'
  | 'state'
  | 'updateModel'
> & { idle: boolean; layoutDirty: boolean; mapping: boolean };

function nodeControlIdle(props: NodeControlInputs) {
  return (
    props.state === 'active' &&
    !props.interactionDisabled &&
    !props.parameters.busy &&
    props.parameters.failures.length === 0 &&
    !props.layout.loading &&
    !props.layout.saving
  );
}

/** The engine's power and mute snapshot, polled only while the workspace is idle or mapping. */
export function useWorkspaceNodeControls(props: NodeControlInputs) {
  const mapping = useMidiMappingActive();
  return useNodeControls(
    props.controlPort,
    props.graphId,
    props.documentRevision,
    nodeControlIdle(props) || mapping,
    props.application,
  );
}

/** The canvas render functions for node header controls and node body content. */
export function useNodeRenderers(
  props: NodeControlInputs &
    Pick<
      WorkspaceViewProps,
      'catalogRevision' | 'failedProcessors' | 'playbackPaused' | 'renderRecorder' | 'updateModel'
    >,
  t: TFunction,
  snapshot: GraphNodeControls | null,
  model: AudioGraphModel | null,
  editors: ReturnType<typeof useProcessorEditors>,
) {
  const idle = nodeControlIdle(props);
  const {
    application,
    catalogRevision,
    controlPort,
    documentRevision,
    failedProcessors,
    graphId,
    interactionDisabled,
    playbackPaused,
    renderRecorder,
    state,
    updateModel,
  } = props;
  const mapping = useMidiMappingActive();
  const layoutDirty = props.layout.dirty;
  const inputs = useMemo<NodeRenderInputs>(
    () => ({
      mapping,
      application,
      catalogRevision,
      controlPort,
      documentRevision,
      failedProcessors,
      graphId,
      idle,
      interactionDisabled,
      layoutDirty,
      playbackPaused,
      renderRecorder,
      state,
      updateModel,
    }),
    [
      mapping,
      application,
      catalogRevision,
      controlPort,
      documentRevision,
      failedProcessors,
      graphId,
      idle,
      interactionDisabled,
      layoutDirty,
      playbackPaused,
      renderRecorder,
      state,
      updateModel,
    ],
  );
  // Recorder edits apply to the committed model current when they are made, not the one
  // their node last rendered with; it is published on commit, never from a discarded render.
  const latestModel = useRef(model);
  useLayoutEffect(() => {
    latestModel.current = model;
  }, [model]);
  return {
    controls: useCallback(
      (node: AudioGraphNode) => nodeSignalControls(inputs, node, t, snapshot),
      [inputs, t, snapshot],
    ),
    content: useCallback(
      (node: AudioGraphNode) => {
        const issue = editors.issues.get(node.id);
        return issue ? (
          <ProcessorEditorAlert
            issue={{ ...issue, name: audioGraphNodeTitle(node) }}
            onRetry={() => editors.retry(node.id)}
          />
        ) : (
          recorderNodeContent(inputs, node, latestModel)
        );
      },
      [inputs, editors.issues, editors.retry],
    ),
  };
}

function recorderNodeContent(
  props: NodeRenderInputs,
  node: AudioGraphNode,
  model: { current: AudioGraphModel | null },
) {
  if (node.runtime?.type !== 'recorder' || !props.graphId) return null;
  return props.renderRecorder?.({
    controlPort: props.controlPort,
    graphId: props.graphId,
    nodeId: node.id,
    name: audioGraphNodeTitle(node),
    expectedRevision: props.documentRevision,
    poweredOn: node.runtime.control?.enabled !== false,
    playbackPaused: props.playbackPaused,
    enabled:
      props.state === 'active' &&
      !props.interactionDisabled &&
      node.runtime.control?.enabled !== false,
    channels: node.inputPorts.length,
    onChannelsChange: (channels) => {
      const current = model.current;
      if (current) props.updateModel(updateRecorderChannels(current, node.id, channels), 'runtime');
    },
  });
}

function nodeRecoveryControl(
  props: NodeRenderInputs,
  node: AudioGraphNode,
  enabled: boolean,
  disabled: boolean,
) {
  const failed =
    node.runtime?.type === 'provider'
      ? props.failedProcessors?.find(
          (candidate) => candidate.graphId === props.graphId && candidate.nodeId === node.id,
        )
      : undefined;
  const retryAvailable = failed !== undefined && props.catalogRevision !== undefined;
  return retryAvailable ? (
    <ProcessorRecoveryControl
      busy={disabled || props.layoutDirty}
      catalogRevision={props.catalogRevision}
      enabled={enabled}
      failedProcessors={props.failedProcessors}
      graphId={props.graphId}
      isProcessor={node.runtime?.type === 'provider'}
      nodeId={node.id}
      nodeName={audioGraphNodeTitle(node)}
      onRetry={(target) => props.application.retryFailedProcessor(target)}
      supported
    />
  ) : null;
}

function nodeSignalControls(
  props: NodeRenderInputs,
  node: AudioGraphNode,
  t: TFunction,
  snapshot: GraphNodeControls | null,
) {
  const control = node.runtime?.control ?? { enabled: true, muted: false };
  const nodeId = runtimeNodeId(node);
  const entry = snapshot?.nodes.find((candidate) => candidate.nodeId === nodeId);
  const capabilities = signalControlVisibility(
    entry?.capabilities,
    control,
    usesRecoveryControls(node.runtime?.type, props.mapping),
  );
  if (!capabilities) return null;
  const disabled = !props.idle;
  const retry = nodeRecoveryControl(props, node, control.enabled, disabled);
  const retryAvailable = retry !== null;
  const graphId = props.graphId;
  if (!graphId || (!entry && !retryAvailable)) return null;
  // Native disabled buttons own interaction gating; the graph queue validates each live command.
  const change = (value: { enabled?: boolean; muted?: boolean }) => {
    void props.application.setNodeControl(graphId, nodeId, value);
  };
  return {
    power: (
      <>
        {retry}
        {capabilities.power ? (
          <NodePowerButton
            enabled={control.enabled}
            disabled={disabled}
            label={t(
              control.enabled ? 'routing.nodeControl.powerOff' : 'routing.nodeControl.powerOn',
              { name: audioGraphNodeTitle(node) },
            )}
            onChange={() => change({ enabled: !control.enabled })}
          />
        ) : null}
      </>
    ),
    mute: capabilities.mute ? (
      <NodeMuteButton
        muted={control.muted}
        disabled={disabled}
        label={t(control.muted ? 'routing.nodeControl.unmute' : 'routing.nodeControl.mute', {
          name: audioGraphNodeTitle(node),
        })}
        onChange={() => change({ muted: !control.muted })}
      />
    ) : null,
  };
}
