import type { RecorderPort } from './RecorderPort';

/** Recorder node capabilities supplied by the workspace to its screen composition. */
export interface RecorderNodeInputs {
  controlPort: RecorderPort;
  graphId: string;
  nodeId: string;
  name?: string;
  expectedRevision: number;
  poweredOn?: boolean;
  playbackPaused?: boolean;
  enabled: boolean;
  channels: number;
  onChannelsChange(channels: number): void;
}
