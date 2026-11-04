import type { ReactNode } from 'react';
import type { EndpointControl, EndpointUpdate } from './AudioNodeEndpointOptions';
import type { AudioNodeMeterBank } from './AudioNodeMeterBanks';
import type { AudioNodeParameterControl } from './AudioNodeParameterGrid';
import type { AudioNodeFaceMode } from './audioNodeLayout';
import type { DeviceAppActivityProps } from './DeviceAppActivity';
import type { DynamicsMeterContract } from './DynamicsMeter';
import type { GraphicEqualizerContract } from './GraphicEqualizer';
import type { StatusTone } from './StatusBadge';

/** A native plug-in editor window a processor node can open. */
export interface ProcessorEditorControl {
  actionLabel: string;
  failureLabel?: string;
  state: 'unavailable' | 'closed' | 'opening' | 'open' | 'failed';
}

/** A node setting chosen from a fixed list, such as a plug-in's channel layout. */
export interface NodeChoiceControl {
  label: string;
  value: string;
  options: readonly { value: string; label: string; description?: string }[];
}

export interface AudioNodeCardProps {
  deviceAppActivity?: DeviceAppActivityProps;
  equalizer?: GraphicEqualizerContract;
  onParametersChange?: (updates: readonly { id: string; value: number }[]) => void;
  headerActions?: ReactNode;
  muteControl?: ReactNode;
  poweredOff?: boolean;
  processingTimeNs?: number;
  content?: ReactNode;
  bypassed?: boolean;
  /** Device presence only; selection and configuration remain available. */
  offline?: boolean;
  category: string;
  channelCount?: number;
  detail?: string;
  endpointControl?: EndpointControl;
  /** Offered in Options when there is more than one choice. */
  channelLayoutControl?: NodeChoiceControl;
  faceHeight?: number;
  faceMode?: AudioNodeFaceMode;
  onFaceHeightChange?: (height: number) => void;
  onContentHeightChange?: (height: number, disclosureHeight: number) => void;
  gainControl?: { label: string; valueDb: number };
  icon: string;
  iconUrl?: string;
  inputCount: number;
  meterBanks?: readonly AudioNodeMeterBank[];
  outputCount: number;
  optionsLabel?: string;
  parameterControls?: readonly AudioNodeParameterControl[];
  dynamics?: DynamicsMeterContract;
  processorEditor?: ProcessorEditorControl;
  showPorts?: boolean;
  selected?: boolean;
  status?: string;
  statusTone?: StatusTone;
  title: string;
  /** Omit when a parent graph owns node selection and keyboard focus. */
  onSelect?: () => void;
  onGainChange?: (gainDb: number) => void;
  onEndpointChange?: (value: EndpointUpdate) => void;
  onChannelLayoutChange?: (value: string) => void;
  onLayoutChange?: () => void;
  onParameterChange?: (parameterId: string, value: number) => void;
  onProcessorEditorRetry?: () => void;
}
