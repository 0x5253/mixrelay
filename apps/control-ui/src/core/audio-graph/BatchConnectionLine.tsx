import { type ConnectionLineComponentProps, getBezierPath } from '@xyflow/react';
import { connectionPreviewPairs } from './connectionPlanning';
import { useAudioPortSelectionContext } from './graphCanvasContexts';
import { type AudioFlowNode, portVerticalOffset } from './reactFlowAdapter';

/** Draws one preview cable per channel pair a drag from the selected ports would connect. */
export function BatchConnectionLine({
  connectionLineStyle,
  connectionStatus,
  fromHandle,
  fromNode,
  fromPosition,
  fromX,
  fromY,
  toHandle,
  toNode,
  toPosition,
  toX,
  toY,
}: ConnectionLineComponentProps<AudioFlowNode>) {
  const selection = useAudioPortSelectionContext();
  const sourceAnchorId = fromHandle.id ?? '';
  const targetAnchorId = toHandle?.id ?? '';
  const previews = connectionPreviewPairs(
    selection.model,
    selection.selectedRef.current,
    fromNode.id,
    sourceAnchorId,
    toNode?.id,
    targetAnchorId,
  );

  return (
    <>
      {previews.map(({ sourcePort, targetPort }, index) => {
        const sourceOffset = portVerticalOffset(fromNode, 'output', sourcePort.id, sourceAnchorId);
        const targetOffset =
          toNode && targetPort
            ? portVerticalOffset(toNode, 'input', targetPort.id, targetAnchorId)
            : 0;
        const [path] = getBezierPath({
          sourcePosition: fromPosition,
          sourceX: fromX,
          sourceY: fromY + sourceOffset,
          targetPosition: toPosition,
          targetX: toX,
          targetY: toY + targetOffset,
        });
        return (
          <path
            className="react-flow__connection-path"
            data-connection-status={connectionStatus ?? undefined}
            d={path}
            fill="none"
            key={`${sourcePort.id}:${targetPort?.id ?? index}`}
            style={connectionLineStyle}
          />
        );
      })}
    </>
  );
}
