import { Checkbox } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';

export function CheckboxField({
  label,
  checked,
  disabled,
  onChange,
}: {
  label: string;
  checked: boolean;
  disabled?: boolean;
  onChange: (checked: boolean) => void;
}) {
  return (
    <Checkbox
      className={styles.checkboxLabel}
      isSelected={checked}
      isDisabled={disabled}
      onChange={onChange}
    >
      {({ isSelected }) => (
        <>
          <span aria-hidden="true" className={styles.checkbox}>
            {isSelected ? '✓' : ''}
          </span>
          <span>{label}</span>
        </>
      )}
    </Checkbox>
  );
}
