import type { XYPosition } from '@xyflow/react';
import type { ReactNode } from 'react';
import type { AudioMeterStore } from '../audio-meter/AudioMeterStore';
import type { ProcessorEditorFailure, ProcessorEditorStatus } from '../domain/control/processors';
import type { EndpointUpdate } from '../ui/AudioNodeEndpointOptions';
import type { HierarchicalMenuItem } from '../ui/HierarchicalMenu';
import type { AudioGraphViewport } from './graphViewport';
import type { AudioGraphModel, AudioGraphNode } from './model';
import type { PlaceGraphNodes } from './nodePlacement';
import type { GraphContextMenuLabels, NodeContextAction } from './useGraphContextMenu';

export interface ProcessorEditorLabels {
  open: string;
  retry: string;
  failure: Record<ProcessorEditorFailure, string>;
}
export interface AudioNodeControlSlots {
  power?: ReactNode;
  mute?: ReactNode;
}
export type AudioGraphChangeKind =
  | 'layout'
  | 'level'
  | 'processor_parameter'
  | 'parameter_group'
  | 'runtime';

export interface AudioGraphCanvasProps {
  nodeContextActions?: (nodeId: string) => readonly NodeContextAction[];
  renderNodeControls?: (node: AudioGraphNode) => AudioNodeControlSlots | null;
  renderNodeContent?: (node: AudioGraphNode) => ReactNode;
  contextMenuLabels?: GraphContextMenuLabels;
  addNodeLabel?: string;
  ariaLabel: string;
  meterStore?: AudioMeterStore;
  model: AudioGraphModel;
  nodeMenuItems?: readonly AudioGraphNodeMenuItem[];
  onAddNode?: (itemId: string, position: XYPosition, placeNodes?: PlaceGraphNodes) => void;
  onNodeMenuOpenedChange?: (opened: boolean) => void;
  onNodeRenameRequest?: (nodeId: string) => void;
  onModelChange: (model: AudioGraphModel, kind: AudioGraphChangeKind) => void;
  onNodeEndpointChange?: (nodeId: string, value: EndpointUpdate) => void;
  /** A choice from the node's `channelLayoutControl`. */
  onNodeChannelLayoutChange?: (nodeId: string, value: string) => void;
  onNodeGainChange?: (nodeId: string, gainDb: number) => void;
  onNodeParameterChange?: (nodeId: string, parameterId: string, value: number) => void;
  onNodeParametersChange?: (
    nodeId: string,
    updates: readonly { id: string; value: number }[],
  ) => void;
  onProcessorEditorRetry?: (nodeId: string) => void;
  processorEditorLabels?: ProcessorEditorLabels;
  processorEditorStatuses?: ReadonlyMap<string, ProcessorEditorStatus>;
  /** Initial or externally restored view; user movement remains locally interactive. */
  viewport?: AudioGraphViewport;
  /** Reports user pan/zoom and insertion reveals, never external viewport restoration. */
  onViewportChange?: (viewport: AudioGraphViewport) => void;
}

export type AudioGraphNodeMenuItem = HierarchicalMenuItem;
