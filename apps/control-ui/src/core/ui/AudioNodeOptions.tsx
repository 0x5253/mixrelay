import { type ReactNode, useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useMidiMappingActive } from '../midi/MidiContext';
import { useMidiControl } from '../midi/useMidiControl';
import { ActionButton } from './ActionButton';
import styles from './AudioNodeCard.module.css';
import { AudioNodeEndpointOptions, type EndpointControl } from './AudioNodeEndpointOptions';
import { type AudioNodeParameterControl, AudioNodeParameterGrid } from './AudioNodeParameterGrid';
import { type ChannelLayoutChoice, partitionParameterControls } from './audioNodeCardModel';
import type { AudioNodeCardProps } from './audioNodeCardTypes';
import { GraphicEqualizer, type GraphicEqualizerContract } from './GraphicEqualizer';
import { NodeOptionsSection } from './NodeOptionsSection';
import { SelectField } from './SelectField';
import { ValueSlider } from './ValueSlider';

type GainControl = NonNullable<AudioNodeCardProps['gainControl']>;

export interface NodeOptionsProps {
  disabled: boolean;
  endpointControl?: EndpointControl;
  channelLayout?: ChannelLayoutChoice;
  gainControl?: GainControl;
  hasOptions: boolean;
  label: string;
  onEndpointChange?: AudioNodeCardProps['onEndpointChange'];
  onGainChange?: AudioNodeCardProps['onGainChange'];
  onParameterChange?: AudioNodeCardProps['onParameterChange'];
  onToggle: () => void;
  opened: boolean;
  parameterControls: readonly AudioNodeParameterControl[];
  equalizer?: GraphicEqualizerContract;
  onParametersChange?: AudioNodeCardProps['onParametersChange'];
}

/** A node's settings: an equalizer graph, or endpoint, gain, parameters and layout. */
export function NodeOptions(props: NodeOptionsProps) {
  if (!props.hasOptions) return null;
  const layoutField = props.channelLayout ? (
    <SelectField
      density="compact"
      label={props.channelLayout.control.label}
      value={props.channelLayout.control.value}
      data={[...props.channelLayout.control.options]}
      disabled={props.disabled}
      onChange={props.channelLayout.onChange}
    />
  ) : null;
  return props.equalizer && props.onParameterChange ? (
    <EqualizerOptions
      {...props}
      equalizer={props.equalizer}
      onParameterChange={props.onParameterChange}
      layoutField={layoutField}
    />
  ) : (
    <StandardOptions {...props} layoutField={layoutField} />
  );
}

function EqualizerOptions({
  disabled,
  equalizer,
  label,
  layoutField,
  onParameterChange,
  onParametersChange,
  onToggle,
  opened,
  parameterControls,
}: NodeOptionsProps & {
  equalizer: GraphicEqualizerContract;
  onParameterChange: NonNullable<AudioNodeCardProps['onParameterChange']>;
  layoutField: ReactNode;
}) {
  return (
    <div className={`${styles.optionsPanel} nodrag nopan nowheel`}>
      <GraphicEqualizer
        contract={equalizer}
        controls={parameterControls}
        disabled={disabled}
        bypassed={Boolean(parameterControls.find((control) => control.id === 'bypass')?.value)}
        onChange={onParameterChange}
        onParametersChange={onParametersChange}
        renderValues={(values) => (
          <NodeOptionsSection label={label} opened={opened} onToggle={onToggle} disabled={disabled}>
            {layoutField}
            {values}
          </NodeOptionsSection>
        )}
      />
    </div>
  );
}

function StandardOptions(props: NodeOptionsProps & { layoutField: ReactNode }) {
  const mapping = useMidiMappingActive();
  const volume = useMidiControl('volume');
  return mapping ? (
    <MidiVolumeControl control={props.gainControl} available={Boolean(props.onGainChange)} />
  ) : (
    <StandardAudioOptions {...props} opened={props.opened || Boolean(volume?.selected)} />
  );
}

function StandardAudioOptions({
  disabled,
  endpointControl,
  gainControl,
  label,
  layoutField,
  onEndpointChange,
  onGainChange,
  onParameterChange,
  onToggle,
  opened,
  parameterControls,
}: NodeOptionsProps & { layoutField: ReactNode }) {
  const inline = parameterControls.length > 0;
  const parameters = partitionParameterControls(parameterControls, Boolean(onParameterChange));
  const controls = (
    <>
      {inline ? null : layoutField}
      {endpointControl && onEndpointChange ? (
        <div inert={disabled && !endpointControl.nativeFormats}>
          <AudioNodeEndpointOptions control={endpointControl} onChange={onEndpointChange} />
        </div>
      ) : null}
      {gainControl && onGainChange ? (
        <NodeGainControl control={gainControl} onChange={onGainChange} disabled={disabled} />
      ) : null}
      {onParameterChange && inline ? (
        <AudioNodeParameterGrid
          controls={parameters.main}
          disabled={disabled}
          onChange={onParameterChange}
        />
      ) : null}
    </>
  );
  if (!inline)
    return (
      <NodeOptionsSection label={label} opened={opened} onToggle={onToggle}>
        {controls}
      </NodeOptionsSection>
    );
  return (
    <>
      <div className={`${styles.optionsPanel} nodrag nopan nowheel`}>{controls}</div>
      {layoutField || parameters.options.length > 0 ? (
        <NodeOptionsSection label={label} opened={opened} onToggle={onToggle} disabled={disabled}>
          {layoutField}
          {onParameterChange && parameters.options.length > 0 ? (
            <AudioNodeParameterGrid
              controls={parameters.options}
              disabled={disabled}
              onChange={onParameterChange}
            />
          ) : null}
        </NodeOptionsSection>
      ) : null}
    </>
  );
}

function MidiVolumeControl({
  control,
  available,
}: {
  control: GainControl | undefined;
  available: boolean;
}) {
  const mapping = useMidiControl('volume');
  const { t } = useTranslation();
  if (!control || !available || !mapping) return null;
  return (
    <ActionButton
      density="compact"
      data-midi-map
      aria-label={mapping.label}
      aria-pressed={mapping.selected}
      onClick={mapping.select}
      onKeyDown={mapping.keyDown}
    >
      {mapping.text || t('midi.actions.volume')}
    </ActionButton>
  );
}

function NodeGainControl({
  control,
  onChange,
  disabled,
}: {
  control: GainControl;
  onChange: (gainDb: number) => void;
  disabled: boolean;
}) {
  const mapping = useMidiControl('volume');
  const [value, setValue] = useState(control.valueDb);
  useEffect(() => setValue(control.valueDb), [control.valueDb]);
  return (
    <span
      className={`${styles.gainControl} nodrag nopan nowheel`}
      data-midi-selected={mapping?.selected || undefined}
    >
      <span className={styles.gainValue}>{value.toFixed(1)} dB</span>
      <ValueSlider
        disabled={disabled}
        max={12}
        min={-60}
        onChange={(nextValue) => {
          setValue(nextValue);
          onChange(nextValue);
        }}
        step={0.5}
        label={control.label}
        value={value}
        valueText={`${value} dB`}
      />
    </span>
  );
}
