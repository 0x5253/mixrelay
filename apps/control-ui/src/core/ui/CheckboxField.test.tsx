import { fireEvent, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CheckboxField } from './CheckboxField';

it('displays the controlled selection and sends a user toggle back to its owner', () => {
  const onChange = vi.fn();
  const view = render(<CheckboxField label="Install Driver" checked onChange={onChange} />);
  const checkbox = view.getByRole('checkbox', { name: 'Install Driver' });
  expect(checkbox).toBeChecked();
  expect(view.getByText('✓')).toHaveAttribute('aria-hidden', 'true');
  fireEvent.click(checkbox);
  expect(onChange).toHaveBeenCalledExactlyOnceWith(false);
  view.rerender(<CheckboxField label="Install Driver" checked={false} onChange={onChange} />);
  expect(checkbox).not.toBeChecked();
  expect(view.queryByText('✓')).not.toBeInTheDocument();
});
