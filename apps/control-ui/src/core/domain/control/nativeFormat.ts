export type NativeSampleFormat = 'float32' | 'signed16' | 'signed24' | 'signed32';

export interface NativeStreamFormat {
  sampleRate: number;
  channels: number;
  sampleFormat: NativeSampleFormat;
}

export interface NativeStreamFormats {
  current: NativeStreamFormat | null;
  supported: NativeStreamFormat[];
}

export interface NativeDeviceFormats {
  render: NativeStreamFormats;
  capture: NativeStreamFormats;
}
