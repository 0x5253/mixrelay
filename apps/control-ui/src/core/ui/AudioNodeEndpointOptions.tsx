import { useEffect, useState } from 'react';
import type { NativeDeviceFormats, NativeSampleFormat } from '../domain/control/nativeFormat';
import { sharedChannelCounts } from '../domain/control/sharedDeviceFormat';
import styles from './AudioNodeCard.module.css';
import { NativeSampleFormatField } from './NativeSampleFormatField';
import { SelectField } from './SelectField';
import { TextField } from './TextField';

const channelOptions = (current: number, allowed: number[]) =>
  [...new Set([...allowed, current])]
    .sort((a, b) => a - b)
    .map((channels) => ({
      label: `${channels}`,
      value: `${channels}`,
      disabled: !allowed.includes(channels),
    }));

export interface EndpointControl {
  nativeFormats?: NativeDeviceFormats;
  nativeFormatLabels?: { bitDepth: string; unknown: string; mixed?: string };
  channelsLabel?: string;
  formatError?: string | null;
  formatsDisabled?: boolean;
  observedRate?: { label: string; value: number };
  sampleRateOptions?: number[];
  rateError?: string | null;
  canRename?: boolean;
  allowedRenderChannels?: number[];
  allowedCaptureChannels?: number[];
  captureChannels: number;
  displayName: string;
  nameLabel: string;
  renderChannels: number;
}

export interface EndpointUpdate {
  sampleRate?: number;
  sampleFormat?: NativeSampleFormat;
  captureChannels: number;
  displayName: string;
  renderChannels: number;
}

interface EndpointOptionsProps {
  control: EndpointControl;
  onChange: (value: EndpointUpdate) => void;
}

export function AudioNodeEndpointOptions({ control, onChange }: EndpointOptionsProps) {
  const [displayName, setDisplayName] = useState(control.displayName);
  useEffect(() => setDisplayName(control.displayName), [control.displayName]);
  const commitName = () => {
    if (control.canRename === false) return;
    const normalized = displayName.trim();
    if (!normalized) {
      setDisplayName(control.displayName);
      return;
    }
    if (normalized !== control.displayName) {
      onChange(endpointValue(control, { displayName: normalized }));
    }
  };
  return (
    <div className={styles.endpointOptions}>
      <TextField
        readOnly={control.canRename === false}
        label={control.nameLabel}
        maxLength={40}
        onBlur={commitName}
        onChange={(event) => setDisplayName(event.currentTarget.value)}
        onKeyDown={(event) => {
          if (event.key === 'Enter') event.currentTarget.blur();
        }}
        value={displayName}
      />
      <EndpointChannels control={control} displayName={displayName} onChange={onChange} />
      <EndpointSampleRate control={control} onChange={onChange} />
      {control.nativeFormats && control.nativeFormatLabels ? (
        <NativeEndpointFormats
          control={control}
          formats={control.nativeFormats}
          labels={control.nativeFormatLabels}
          onChange={onChange}
        />
      ) : null}
    </div>
  );
}

function EndpointChannels({
  control,
  displayName,
  onChange,
}: EndpointOptionsProps & { displayName: string }) {
  const hasRender = control.renderChannels > 0;
  const hasCapture = control.captureChannels > 0;
  const current =
    hasRender && hasCapture && control.renderChannels !== control.captureChannels
      ? null
      : hasRender
        ? control.renderChannels
        : control.captureChannels;
  const allowed = sharedChannelCounts(
    control.allowedRenderChannels ?? Array.from({ length: 64 }, (_, index) => index + 1),
    control.allowedCaptureChannels ?? Array.from({ length: 64 }, (_, index) => index + 1),
    hasRender,
    hasCapture,
  );
  return (
    <SelectField
      label={control.channelsLabel ?? 'Channels'}
      placeholder={control.nativeFormatLabels?.mixed ?? 'Mixed'}
      data={channelOptions(current ?? 0, allowed).filter((option) => option.value !== '0')}
      value={current === null ? null : String(current)}
      disabled={allowed.length === 0 || (allowed.length === 1 && allowed[0] === current)}
      onChange={(value) => {
        // The select disables observed counts outside the allowed device capability.
        onChange(
          endpointValue(control, {
            displayName,
            renderChannels: hasRender ? Number(value) : 0,
            captureChannels: hasCapture ? Number(value) : 0,
          }),
        );
      }}
    />
  );
}

function EndpointSampleRate({ control, onChange }: EndpointOptionsProps) {
  return (
    <>
      {control.observedRate && control.sampleRateOptions?.length ? (
        <SelectField
          label={control.observedRate.label}
          error={control.rateError}
          data={[...new Set([...control.sampleRateOptions, control.observedRate.value])].map(
            (value) => ({ value: String(value), label: `${value / 1000} kHz` }),
          )}
          value={String(control.observedRate.value)}
          disabled={
            control.sampleRateOptions.length === 1 &&
            control.sampleRateOptions[0] === control.observedRate.value
          }
          onChange={(value) => {
            onChange({ ...endpointValue(control, {}), sampleRate: Number(value) });
          }}
        />
      ) : control.observedRate ? (
        <TextField
          readOnly
          label={control.observedRate.label}
          value={`${control.observedRate.value / 1000} kHz`}
        />
      ) : null}
    </>
  );
}

function NativeEndpointFormats({
  control,
  formats,
  labels,
  onChange,
}: EndpointOptionsProps & {
  formats: NativeDeviceFormats;
  labels: NonNullable<EndpointControl['nativeFormatLabels']>;
}) {
  return (
    <NativeSampleFormatField
      formats={formats}
      render={control.renderChannels > 0}
      capture={control.captureChannels > 0}
      label={labels.bitDepth}
      unknownLabel={labels.unknown}
      mixedLabel={labels.mixed}
      disabled={control.formatsDisabled}
      error={control.formatError}
      onChange={(sampleFormat) => onChange({ ...endpointValue(control, {}), sampleFormat })}
    />
  );
}

function endpointValue(
  control: EndpointControl,
  update: Partial<Pick<EndpointControl, 'captureChannels' | 'displayName' | 'renderChannels'>>,
) {
  return {
    captureChannels: update.captureChannels ?? control.captureChannels,
    displayName: update.displayName ?? control.displayName,
    renderChannels: update.renderChannels ?? control.renderChannels,
  };
}
