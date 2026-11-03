import type { NativeDeviceFormats, NativeSampleFormat } from './nativeFormat';

/** Sample formats in the order choices are offered. */
export const nativeSampleFormats: readonly NativeSampleFormat[] = [
  'signed16',
  'signed24',
  'signed32',
  'float32',
];

/** The native shared PCM command preserves each current rate/channel tuple. */
export function sharedDevicePcm(formats: NativeDeviceFormats, render: boolean, capture: boolean) {
  const streams = [render ? formats.render : null, capture ? formats.capture : null].filter(
    (stream) => stream !== null,
  );
  const current = streams[0]?.current?.sampleFormat ?? null;
  const value =
    current && streams.every((stream) => stream.current?.sampleFormat === current) ? current : null;
  const options = nativeSampleFormats.filter(
    (sampleFormat) =>
      streams.length > 0 &&
      streams.every(
        (stream) =>
          stream.current &&
          stream.supported.some(
            (format) =>
              format.sampleRate === stream.current?.sampleRate &&
              format.channels === stream.current.channels &&
              format.sampleFormat === sampleFormat,
          ),
      ),
  );
  const mixed =
    streams.length > 1 && streams.every((stream) => stream.current !== null) && value === null;
  return { value, options, mixed };
}

export function sharedChannelCounts(
  render: number[],
  capture: number[],
  hasRender = true,
  hasCapture = true,
) {
  if (!hasRender && !hasCapture) return [];
  const choices = hasRender ? render : capture;
  return [...new Set(choices)]
    .filter((count) => !hasCapture || !hasRender || capture.includes(count))
    .sort((a, b) => a - b);
}
