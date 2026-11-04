import type { ComponentPropsWithoutRef, ReactNode } from 'react';
import { TextField as AriaTextField, Input, Label, Text } from 'react-aria-components';
import styles from '../theme/ComponentTheme.module.css';
import type { NoAppearanceOverrides } from './ActionButton';

type TextFieldProps = Pick<
  ComponentPropsWithoutRef<'input'>,
  | 'value'
  | 'defaultValue'
  | 'placeholder'
  | 'disabled'
  | 'readOnly'
  | 'required'
  | 'maxLength'
  | 'minLength'
  | 'name'
  | 'id'
  | 'autoComplete'
  | 'autoFocus'
  | 'onChange'
  | 'onBlur'
  | 'onFocus'
  | 'onKeyDown'
  | 'aria-label'
  | 'aria-describedby'
> &
  NoAppearanceOverrides & {
    label?: string;
    description?: ReactNode;
    error?: ReactNode;
    'data-autofocus'?: boolean;
  };

export function TextField({
  label,
  description,
  error,
  disabled,
  readOnly,
  required,
  defaultValue,
  ...props
}: TextFieldProps) {
  return (
    <AriaTextField
      className={styles.field}
      isDisabled={disabled}
      isReadOnly={readOnly}
      isRequired={required}
      isInvalid={Boolean(error)}
      defaultValue={defaultValue === undefined ? undefined : String(defaultValue)}
      aria-label={props['aria-label']}
    >
      {label ? <Label className={styles.fieldLabel}>{label}</Label> : null}
      <Input
        {...props}
        className={styles.input}
        aria-invalid={Boolean(error) || undefined}
        autoFocus={props.autoFocus || props['data-autofocus']}
      />
      {description ? (
        <Text slot="description" className={styles.fieldDescription}>
          {description}
        </Text>
      ) : null}
      {error ? (
        <Text slot="errorMessage" role="alert" className={styles.fieldError}>
          {error}
        </Text>
      ) : null}
    </AriaTextField>
  );
}
