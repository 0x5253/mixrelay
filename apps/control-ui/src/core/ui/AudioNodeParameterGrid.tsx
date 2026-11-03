import { useEffect, useState } from 'react';
import styles from './AudioNodeCard.module.css';
import { ParameterField, type ParameterFieldControl } from './ParameterField';
import { ValueSlider } from './ValueSlider';

/** A node parameter: the shared field contract plus where it sits on the card. */
export interface AudioNodeParameterControl extends ParameterFieldControl {
  section?: 'main' | 'options';
  precise?: boolean;
}

export function AudioNodeParameterGrid({
  controls,
  disabled,
  onChange,
}: {
  controls: readonly AudioNodeParameterControl[];
  disabled: boolean;
  onChange: (parameterId: string, value: number) => void;
}) {
  return (
    <div className={styles.parameterList} inert={disabled}>
      {parameterControlGroups(controls).map((group) => (
        <div
          key={group.id}
          className={group.knobs ? styles.knobGroup : undefined}
          data-columns={Math.min(3, group.controls.length)}
        >
          {group.label ? <span className={styles.parameterGroupLabel}>{group.label}</span> : null}
          {group.controls.map((control) => (
            <NodeParameterControl
              control={control}
              key={control.id}
              disabled={disabled}
              onChange={(value) => onChange(control.id, value)}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

function parameterControlGroups(controls: readonly AudioNodeParameterControl[]) {
  const groups: {
    id: string;
    label?: string;
    knobs: boolean;
    controls: (typeof controls)[number][];
  }[] = [];
  for (const control of controls) {
    const id =
      control.presentation === 'knob' && !control.choices
        ? (control.group?.id ?? 'knobs')
        : control.id;
    const last = groups.at(-1);
    if (last?.id === id) last.controls.push(control);
    else
      groups.push({
        id,
        label: control.group?.label,
        knobs: control.presentation === 'knob' && !control.choices,
        controls: [control],
      });
  }
  return groups;
}

function NodeParameterControl({
  control,
  onChange,
  disabled,
}: {
  control: AudioNodeParameterControl;
  onChange: (value: number) => void;
  disabled?: boolean;
}) {
  const [value, setValue] = useState(control.value);
  useEffect(() => setValue(control.value), [control.value]);
  if (control.precise || control.choices || control.scale) {
    return <ParameterField control={control} onChange={onChange} disabled={disabled} />;
  }
  return (
    <div className={`${styles.parameterControl} nodrag nopan nowheel`}>
      <span className={styles.parameterLabel}>{control.label}</span>
      <span className={styles.parameterValue}>
        {value.toFixed(control.step < 1 ? 1 : 0)} {control.suffix}
      </span>
      <ValueSlider
        disabled={disabled}
        max={control.maximum}
        min={control.minimum}
        onChange={(nextValue) => {
          setValue(nextValue);
          onChange(nextValue);
        }}
        step={control.step}
        label={control.label}
        value={value}
        valueText={`${value} ${control.suffix}`}
      />
    </div>
  );
}
