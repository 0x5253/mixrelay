export type RecorderEncoding = 'pcm16' | 'pcm24' | 'float32';

export interface RecorderRequest {
  action: 'record' | 'pause' | 'stop';
}

export interface RecorderSettings {
  folder: string;
  sampleRate: number | null;
  encoding: RecorderEncoding;
}

export interface RecorderStatus {
  state: 'stopped' | 'recording' | 'paused' | 'failed';
  path: string | null;
  sampleRate: number | null;
  encoding: RecorderEncoding | null;
  frames: number;
  error: string | null;
}
