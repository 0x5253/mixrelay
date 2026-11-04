import {
  Handle,
  type NodeProps,
  NodeResizeControl,
  Position,
  useConnection,
  useUpdateNodeInternals,
} from '@xyflow/react';
import { type ReactNode, useLayoutEffect } from 'react';
import { useTranslation } from 'react-i18next';
import { MidiNodeContext } from '../midi/MidiContext';
import { AudioNodeCard } from '../ui/AudioNodeCard';
import { audioNodePortTop } from '../ui/audioNodeLayout';
import { NodeOptionsIdContext } from '../ui/nodeOptionsState';
import { Tooltip } from '../ui/Tooltip';
import styles from './AudioGraphCanvas.module.css';
import { useNodeCallbacks, useNodeDisclosureHeights } from './graphCanvasContexts';
import {
  type AudioGraphNode,
  type AudioGraphPort,
  audioGraphNodeTitle,
  graphNodeMinimumWidth,
} from './model';
import { audioFlowNodeWidth, processorEditorFace } from './nodeViewModel';
import { namedAudioPorts } from './portNames';
import type { AudioFlowNode } from './reactFlowAdapter';
import { graphNodeFaceMode, useAudioNodeLayout } from './useAudioNodeLayout';

type AudioPortSide = 'input' | 'output';

/** Binds a node id to an optional per-node callback, keeping it absent when unset. */
function bindNode<Args extends unknown[]>(
  nodeId: string,
  callback: ((nodeId: string, ...args: Args) => void) | undefined,
) {
  return callback ? (...args: Args) => callback(nodeId, ...args) : undefined;
}

/** Keeps the canvas's record of open disclosure heights in step with this node. */
function useDisclosureHeight(nodeId: string, height: number) {
  const disclosureHeights = useNodeDisclosureHeights();
  useLayoutEffect(() => {
    if (height > 0) disclosureHeights.set(nodeId, height);
    else disclosureHeights.delete(nodeId);
    return () => {
      disclosureHeights.delete(nodeId);
    };
  }, [disclosureHeights, nodeId, height]);
}

export function AudioFlowNodeView({ data, selected }: NodeProps<AudioFlowNode>) {
  const { t } = useTranslation();
  const { graphNode } = data;
  const callbacks = useNodeCallbacks();
  const { processorEditors } = callbacks;
  const controls = callbacks.renderControls?.(graphNode);
  const updateNodeInternals = useUpdateNodeInternals();
  const editorStatus = processorEditors?.statuses.get(graphNode.id);
  const editorWidth = editorStatus?.state === 'open' ? editorStatus.width : null;
  const layout = useAudioNodeLayout(graphNode, {
    width: editorWidth,
    height: editorStatus?.height ?? null,
  });
  useDisclosureHeight(graphNode.id, layout.disclosureHeight);
  const { appearance } = graphNode;
  return (
    <MidiNodeScope node={graphNode}>
      <NodeOptionsIdContext.Provider value={graphNode.id}>
        <div
          className={styles.node}
          data-node-type={graphNode.runtime?.type}
          data-equalizer={Boolean(appearance.equalizer) || undefined}
          data-dynamics={Boolean(appearance.dynamics) || undefined}
          style={{
            width: audioFlowNodeWidth(graphNode, editorWidth),
            height: layout.height,
            minHeight: layout.minHeight,
          }}
        >
          <NodePortHandles node={graphNode} geometry={layout.ports} side="input" />
          <AudioNodeCard
            {...appearance}
            title={audioGraphNodeTitle(graphNode)}
            optionsLabel={appearance.optionsLabel ?? t('routing.nodes.options')}
            content={callbacks.renderContent?.(graphNode)}
            headerActions={controls?.power}
            muteControl={controls?.mute}
            poweredOff={graphNode.runtime?.control?.enabled === false}
            faceHeight={layout.faceHeight}
            faceMode={graphNodeFaceMode(graphNode)}
            onFaceHeightChange={layout.onFaceHeightChange}
            onContentHeightChange={layout.onContentHeightChange}
            inputCount={graphNode.inputPorts.length}
            onGainChange={bindNode(graphNode.id, callbacks.gain)}
            onChannelLayoutChange={bindNode(graphNode.id, callbacks.channelLayout)}
            onEndpointChange={bindNode(graphNode.id, callbacks.endpoint)}
            onLayoutChange={() => updateNodeInternals(graphNode.id)}
            onParameterChange={bindNode(graphNode.id, callbacks.parameter)}
            onParametersChange={bindNode(graphNode.id, callbacks.parameters)}
            onProcessorEditorRetry={
              appearance.hasProcessorEditor
                ? bindNode(graphNode.id, processorEditors?.onRetry)
                : undefined
            }
            outputCount={graphNode.outputPorts.length}
            processorEditor={processorEditorFace(
              graphNode.id,
              appearance.hasProcessorEditor,
              processorEditors,
            )}
            selected={selected}
            showPorts={false}
          />
          <NodePortHandles node={graphNode} geometry={layout.ports} side="output" />
          <NodeResizeGrip
            minHeight={layout.minHeight}
            minWidth={graphNodeMinimumWidth(graphNode)}
          />
        </div>
      </NodeOptionsIdContext.Provider>
    </MidiNodeScope>
  );
}

function MidiNodeScope({ node, children }: { node: AudioGraphNode; children: ReactNode }) {
  return (
    <MidiNodeContext.Provider
      value={{
        nodeId: node.runtime?.binding?.nodeId ?? node.id,
        nodeName: audioGraphNodeTitle(node),
      }}
    >
      {children}
    </MidiNodeContext.Provider>
  );
}

function NodePortHandles({
  node,
  geometry,
  side,
}: {
  node: AudioGraphNode;
  geometry: Parameters<typeof audioNodePortTop>[0];
  side: AudioPortSide;
}) {
  const ports = side === 'input' ? node.inputPorts : node.outputPorts;
  return namedAudioPorts(ports).map(({ port, name, channel }, index) => (
    <AudioPortHandle
      channel={channel}
      name={name}
      top={audioNodePortTop(geometry, index, ports.length)}
      key={port.id}
      nodeId={node.id}
      port={port}
      side={side}
    />
  ));
}

function NodeResizeGrip({ minHeight, minWidth }: { minHeight: number; minWidth: number }) {
  return (
    <NodeResizeControl
      className={`${styles.resizeHandle} nodrag nopan`}
      // Keep the independent translate reset out of CSS transform lowering.
      style={{ translate: 'none' }}
      position="bottom-right"
      minWidth={minWidth}
      minHeight={minHeight}
      maxWidth={4096}
      maxHeight={4096}
      autoScale={false}
    >
      <svg aria-hidden="true" width="12" height="12" viewBox="0 0 12 12">
        <path d="m3 10 7-7M7 10l3-3" fill="none" stroke="currentColor" />
      </svg>
    </NodeResizeControl>
  );
}

function AudioPortHandle({
  top,
  channel: channelNumber,
  name,
  nodeId,
  port,
  side,
}: {
  top: number;
  channel: number;
  name: string;
  nodeId: string;
  port: AudioGraphPort;
  side: AudioPortSide;
}) {
  const isConnectionTarget = useConnection(
    (connection) =>
      connection.inProgress &&
      connection.toHandle?.nodeId === nodeId &&
      connection.toHandle.id === port.id,
  );

  return (
    <Tooltip
      kind="readout"
      nonInteractive
      label={`CH ${channelNumber}`}
      offset={7}
      forceOpen={isConnectionTarget}
      delay={isConnectionTarget ? 0 : 160}
      position={side === 'input' ? 'left' : 'right'}
    >
      {/* A named graphic: ports stay out of Tab order but expose node direction and channel. */}
      <Handle
        role="img"
        aria-label={`${name}, CH ${channelNumber}`}
        className={`${styles.handle} ${side === 'input' ? styles.inputHandle : ''}`}
        data-audio-node-id={nodeId}
        data-audio-port-id={port.id}
        data-audio-port-side={side}
        data-connection-target={isConnectionTarget || undefined}
        id={port.id}
        position={side === 'input' ? Position.Left : Position.Right}
        style={{
          top: `${top}px`,
        }}
        type={side === 'input' ? 'target' : 'source'}
      />
    </Tooltip>
  );
}
