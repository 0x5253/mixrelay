import {
  Button,
  Label,
  ListBox,
  ListBoxItem,
  Popover,
  Select,
  SelectValue,
  Text,
} from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';
import type { NoAppearanceOverrides } from './ActionButton';

export interface SelectFieldProps extends NoAppearanceOverrides {
  label: string;
  value: string | null;
  placeholder?: string;
  data: { value: string; label: string; description?: string; disabled?: boolean }[];
  disabled?: boolean;
  error?: string | null;
  description?: string;
  density?: 'regular' | 'compact';
  labelPlacement?: 'above' | 'inline';
  /** Takes focus on mount, e.g. as a dialog's first field. */
  autoFocus?: boolean;
  onChange: (value: string) => void;
}

/** A select-only field: current readback stays visible until its owner confirms a change. */
export function SelectField({
  label,
  value,
  placeholder,
  data,
  disabled,
  error,
  description,
  density = 'regular',
  labelPlacement = 'above',
  autoFocus,
  onChange,
}: SelectFieldProps) {
  const selectedDescription = description ?? data.find((item) => item.value === value)?.description;
  return (
    <Select
      className={styles.field}
      data-density={density}
      data-label-placement={labelPlacement}
      selectedKey={value}
      disabledKeys={data.filter((item) => item.disabled).map((item) => item.value)}
      // Every item ID below is a string; this select does not allow deselection.
      onSelectionChange={(key) => onChange(key as string)}
      placeholder={placeholder}
      autoFocus={autoFocus}
      isDisabled={disabled}
      isInvalid={Boolean(error)}
    >
      <Label className={styles.fieldLabel}>{label}</Label>
      <Button className={styles.selectButton}>
        <SelectValue />
        <span aria-hidden="true" className={styles.selectChevron}>
          ⌄
        </span>
      </Button>
      <Popover
        style={{ zIndex: 'var(--layer-floating)' }}
        className={styles.selectPopover}
        placement="bottom start"
        offset={4}
      >
        <ListBox className={styles.listBox} items={data}>
          {(item) => (
            <ListBoxItem
              id={item.value}
              textValue={item.label}
              isDisabled={item.disabled}
              onPressStart={(event) => {
                if (event.pointerType === 'keyboard' && item.value === value) onChange(item.value);
              }}
              onPress={(event) => {
                if (event.pointerType !== 'keyboard' && item.value === value) onChange(item.value);
              }}
              className={styles.option}
            >
              {({ isSelected }) => (
                <>
                  <span>{item.label}</span>
                  <span aria-hidden="true">
                    <span data-selected={isSelected || undefined} />
                  </span>
                </>
              )}
            </ListBoxItem>
          )}
        </ListBox>
      </Popover>
      {selectedDescription ? (
        <Text slot="description" className={styles.fieldDescription}>
          {selectedDescription}
        </Text>
      ) : null}
      {error ? (
        <Text slot="errorMessage" role="alert" className={styles.fieldError}>
          {error}
        </Text>
      ) : null}
    </Select>
  );
}
