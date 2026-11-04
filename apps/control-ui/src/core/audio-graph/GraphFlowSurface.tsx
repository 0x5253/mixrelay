import {
  type AriaLabelConfig,
  ConnectionMode,
  ReactFlow,
  type ReactFlowProps,
} from '@xyflow/react';
import { memo, type RefObject, useCallback, useMemo } from 'react';
import { useTranslation } from 'react-i18next';
import { useMidiMappingActive } from '../midi/MidiContext';
import { platformModifier } from '../ui/platformShortcut';
import { AudioFlowNodeView } from './AudioFlowNodeView';
import { BatchConnectionLine } from './BatchConnectionLine';
import type { AudioFlowEdge, AudioFlowNode } from './reactFlowAdapter';

export const fitViewOptions = { padding: 0.15, maxZoom: 1 };
const nodeTypes = { audio: memo(AudioFlowNodeView) };
const deleteKeyCode = ['Backspace', 'Delete'];
const panMouseButtons = [1];
const snapGrid: [number, number] = [12, 12];

type ArrowDirection = 'up' | 'down' | 'left' | 'right';

/** React Flow's built-in keyboard and live-region text follows the app language. */
function useGraphAriaLabels(): Partial<AriaLabelConfig> {
  const { t } = useTranslation();
  return useMemo(
    () => ({
      'node.a11yDescription.default': t('routing.a11y.nodeDescriptionStatic'),
      'node.a11yDescription.keyboardDisabled': t('routing.a11y.nodeDescription'),
      'node.a11yDescription.ariaLiveMessage': ({ direction, x, y }) =>
        t('routing.a11y.nodeMoved', {
          direction: t(`routing.a11y.directions.${direction as ArrowDirection}`),
          x,
          y,
        }),
      'edge.a11yDescription.default': t('routing.a11y.edgeDescription'),
    }),
    [t],
  );
}

/** The configured React Flow; MIDI mapping mode suspends every graph edit and gesture. */
export function GraphFlowSurface({
  focusRoot,
  ...props
}: ReactFlowProps<AudioFlowNode, AudioFlowEdge> & { focusRoot: RefObject<HTMLElement | null> }) {
  const ariaLabelConfig = useGraphAriaLabels();
  const mapping = useMidiMappingActive();
  // React Flow's deletion keys listen on the whole document. Keeping the listener mounted
  // preserves its key tracking; deletion itself proceeds only while focus is in the graph,
  // so Delete in a dialog or elsewhere never removes the selection behind it.
  const onBeforeDelete = useCallback(
    () => Promise.resolve(Boolean(!mapping && focusRoot.current?.contains(document.activeElement))),
    [focusRoot, mapping],
  );
  return (
    <ReactFlow<AudioFlowNode, AudioFlowEdge>
      {...props}
      ariaLabelConfig={ariaLabelConfig}
      colorMode="dark"
      proOptions={{ hideAttribution: true }}
      connectionLineComponent={BatchConnectionLine}
      connectionMode={ConnectionMode.Strict}
      zIndexMode="manual"
      deleteKeyCode={deleteKeyCode}
      onBeforeDelete={onBeforeDelete}
      onNodeClick={mapping ? (event) => event.stopPropagation() : props.onNodeClick}
      onNodeDoubleClick={mapping ? undefined : props.onNodeDoubleClick}
      nodesDraggable={!mapping}
      nodesConnectable={!mapping}
      nodesFocusable={!mapping}
      edgesFocusable={!mapping}
      elementsSelectable={!mapping}
      disableKeyboardA11y={mapping}
      edgesReconnectable={!mapping}
      fitViewOptions={fitViewOptions}
      // Match the persisted layout contract; React Flow's defaults would clamp restored views.
      minZoom={0.05}
      maxZoom={8}
      multiSelectionKeyCode="Shift"
      nodeTypes={nodeTypes}
      panActivationKeyCode={mapping ? null : platformModifier()}
      panOnDrag={mapping ? false : panMouseButtons}
      zoomOnScroll={!mapping}
      zoomOnPinch={!mapping}
      zoomOnDoubleClick={!mapping}
      reconnectRadius={18}
      selectionOnDrag={!mapping}
      snapGrid={snapGrid}
      snapToGrid
    />
  );
}
