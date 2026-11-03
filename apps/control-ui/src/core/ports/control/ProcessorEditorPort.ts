import type {
  ProcessorEditorFrame,
  ProcessorEditorInput,
  ProcessorEditorStatus,
  ProcessorParameterUpdateResponse,
} from '../../domain/control/processors';

/** Plug-in editors and live parameter values of processor nodes. */
export interface ProcessorEditorPort {
  getProcessorEditor(nodeId: string): Promise<ProcessorEditorStatus>;
  setProcessorEditor(
    nodeId: string,
    open: boolean,
    expectedRevision: number,
  ): Promise<ProcessorEditorStatus>;
  getProcessorEditorFrame(nodeId: string): Promise<ProcessorEditorFrame>;
  sendProcessorEditorInput(nodeId: string, input: ProcessorEditorInput): Promise<void>;
  setProcessorParameter(
    nodeId: string,
    parameterId: number,
    value: number,
    expectedRevision: number,
  ): Promise<ProcessorParameterUpdateResponse>;
}
