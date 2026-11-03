import type { NativeDeviceFormats, NativeSampleFormat } from '../domain/control/nativeFormat';
import { sharedDevicePcm } from '../domain/control/sharedDeviceFormat';
import { SelectField } from './SelectField';

const sampleFormatLabels: Record<NativeSampleFormat, string> = {
  signed16: '16-bit PCM',
  signed24: '24-bit PCM',
  signed32: '32-bit PCM',
  float32: '32-bit Float',
};

export function NativeSampleFormatField({
  formats,
  render,
  capture,
  label,
  unknownLabel,
  mixedLabel,
  disabled,
  error,
  onChange,
}: {
  formats: NativeDeviceFormats;
  render: boolean;
  capture: boolean;
  label: string;
  unknownLabel: string;
  mixedLabel?: string;
  disabled?: boolean;
  error?: string | null;
  onChange: (format: NativeSampleFormat) => void;
}) {
  const { value, options, mixed } = sharedDevicePcm(formats, render, capture);
  const choices = [...new Set([...options, ...(value ? [value] : [])])];
  return (
    <SelectField
      label={label}
      placeholder={mixed ? (mixedLabel ?? unknownLabel) : unknownLabel}
      value={value}
      error={error}
      data={choices.map((format) => ({
        value: format,
        label: sampleFormatLabels[format],
        disabled: !options.includes(format),
      }))}
      disabled={disabled || options.length === 0 || (options.length === 1 && options[0] === value)}
      onChange={(next) => {
        if (next && next !== value && options.includes(next as NativeSampleFormat))
          onChange(next as NativeSampleFormat);
      }}
    />
  );
}
