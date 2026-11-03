import styles from './LevelControl.module.css';
import { ValueSlider } from './ValueSlider';

export function LevelControl({
  label,
  max,
  min,
  onChange,
  suffix,
  value,
  disabled,
}: {
  label: string;
  max: number;
  min: number;
  onChange: (value: number) => void;
  suffix: string;
  value: number;
  disabled?: boolean;
}) {
  return (
    <div className={styles.root}>
      <span className={styles.label}>
        <span>{label}</span>
        <span className={styles.value}>
          {value} {suffix}
        </span>
      </span>
      <ValueSlider
        label={label}
        value={value}
        min={min}
        max={max}
        disabled={disabled}
        onChange={onChange}
        valueText={`${value} ${suffix}`}
      />
    </div>
  );
}
