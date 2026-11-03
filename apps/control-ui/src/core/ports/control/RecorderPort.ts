import type {
  RecorderRequest,
  RecorderSettings,
  RecorderStatus,
} from '../../domain/control/recorder';

/** Recorder nodes: transport, saved preferences and the folder chooser. */
export interface RecorderPort {
  recorderStatus(graphId: string, nodeId: string): Promise<RecorderStatus>;
  controlRecorder(
    graphId: string,
    nodeId: string,
    request: RecorderRequest,
    expectedRevision: number,
  ): Promise<RecorderStatus>;
  recordingSettings(graphId: string, nodeId: string): Promise<RecorderSettings>;
  setRecordingSettings(
    graphId: string,
    nodeId: string,
    settings: RecorderSettings,
    expectedRevision: number,
  ): Promise<RecorderSettings>;
  selectRecordingFolder?(options: { defaultPath?: string; title?: string }): Promise<string | null>;
}
