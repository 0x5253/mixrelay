import { useId } from 'react';
import { Input, Label, NumberField } from 'react-aria-components';
import theme from '../theme/ComponentTheme.module.css';
import { Knob } from './Knob';
import { NumericValueControl } from './NumericValueControl';
import styles from './ParameterField.module.css';
import { SelectField } from './SelectField';
import { ValueSlider } from './ValueSlider';

export interface ParameterFieldControl {
  id: string;
  label: string;
  description?: string;
  minimum: number;
  maximum: number;
  step: number;
  suffix: string;
  value: number;
  scale?: 'linear' | 'logarithmic';
  choices?: readonly { value: number; label: string; description?: string }[];
  presentation?: 'slider' | 'knob';
  group?: { id: string; label: string };
  endpoints?: { minimum: string; maximum: string };
}

/** Core owns precise entry, optional discrete choices and continuous audio adjustment. */
export function ParameterField({
  control,
  disabled,
  onChange,
}: {
  control: ParameterFieldControl;
  disabled?: boolean;
  onChange: (value: number) => void;
}) {
  const unitId = useId();
  if (control.choices) {
    return (
      <SelectField
        label={control.label}
        description={control.description}
        value={String(control.value)}
        density="compact"
        disabled={disabled}
        data={control.choices.map((choice) => ({
          value: String(choice.value),
          label: choice.label,
          description: choice.description,
        }))}
        onChange={(value) => onChange(Number(value))}
      />
    );
  }
  return (
    <div
      className={`${styles.root} ${control.presentation === 'knob' ? styles.knobField : ''} nodrag nopan nowheel nokey`}
    >
      {control.presentation === 'knob' ? (
        <>
          <span className={styles.knobLabel}>{control.label}</span>
          <Knob
            label={control.label}
            value={control.value}
            min={control.minimum}
            max={control.maximum}
            step={control.step}
            scale={control.scale}
            disabled={disabled}
            endpoints={control.endpoints}
            valueText={`${control.value} ${control.suffix}${control.endpoints ? ` (${control.endpoints.minimum} ${control.minimum} / ${control.endpoints.maximum} ${control.maximum})` : ''}`.trim()}
            onChange={onChange}
          />
        </>
      ) : null}
      {control.presentation === 'knob' ? (
        <NumericValueControl
          density="compact"
          label={control.label}
          value={control.value}
          minimum={control.minimum}
          maximum={control.maximum}
          step={control.step}
          scale={control.scale}
          suffix={control.suffix}
          disabled={disabled}
          onChange={onChange}
        />
      ) : (
        <NumberField
          className={styles.number}
          value={control.value}
          minValue={control.minimum}
          maxValue={control.maximum}
          step={control.step}
          isDisabled={disabled}
          formatOptions={{ maximumFractionDigits: 3, useGrouping: false }}
          onChange={(value) => {
            if (Number.isFinite(value)) onChange(value);
          }}
        >
          <Label className={theme.fieldLabel}>{control.label}</Label>
          <div className={styles.entry}>
            <Input className={theme.input} aria-describedby={control.suffix ? unitId : undefined} />
            <span className={styles.unit} id={unitId}>
              {control.suffix}
            </span>
          </div>
        </NumberField>
      )}
      {control.presentation !== 'knob' ? (
        <ValueSlider
          label={control.label}
          value={control.value}
          min={control.minimum}
          max={control.maximum}
          step={control.step}
          scale={control.scale}
          disabled={disabled}
          valueText={`${control.value} ${control.suffix}`}
          onChange={onChange}
        />
      ) : null}
    </div>
  );
}
