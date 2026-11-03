import { SelectField } from './SelectField';

export function InlineSelect({
  title,
  ...props
}: {
  label: string;
  value: string;
  data: { value: string; label: string }[];
  disabled?: boolean;
  title?: string;
  onChange: (value: string) => void;
}) {
  return (
    <div title={title}>
      <SelectField {...props} density="compact" labelPlacement="inline" />
    </div>
  );
}
