import { fireEvent, render, screen } from '@testing-library/react';
import { assert, describe, expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { NameDialog } from './NameDialog';

const props = {
  title: 'Rename',
  initialName: 'Firefox',
  nameLabel: 'Name',
  createLabel: 'Save',
  enabled: true,
  onCancel: vi.fn(),
};

function submit(input: HTMLElement) {
  const form = input.closest('form');
  assert(form);
  fireEvent.submit(form);
}

describe('name dialog creation and display-label editing', () => {
  it('keeps the existing non-empty, trimmed creation contract and default field length', () => {
    const save = vi.fn();
    render(
      <MixRelayProvider>
        <NameDialog {...props} onCreate={save} />
      </MixRelayProvider>,
    );
    const input = screen.getByRole('textbox', { name: 'Name' });
    expect(input).toHaveAttribute('maxlength', '40');
    fireEvent.change(input, { target: { value: '  ' } });
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
    submit(input);
    expect(save).not.toHaveBeenCalled();
    fireEvent.change(input, { target: { value: '  Video  ' } });
    submit(input);
    expect(save).toHaveBeenCalledWith('Video');
  });
  it('allows an empty display-label reset and describes the original name', () => {
    const save = vi.fn();
    render(
      <MixRelayProvider>
        <NameDialog
          {...props}
          allowEmpty
          maxLength={128}
          description="Original: Firefox"
          onCreate={save}
        />
      </MixRelayProvider>,
    );
    const input = screen.getByRole('textbox', { name: 'Name' });
    expect(input).toHaveAttribute('maxlength', '128');
    expect(input).toHaveAccessibleDescription('Original: Firefox');
    fireEvent.change(input, { target: { value: '' } });
    expect(screen.getByRole('button', { name: 'Save' })).toBeEnabled();
    submit(input);
    expect(save).toHaveBeenCalledWith('');
  });
  it('does not submit while disabled even when empty labels are allowed', () => {
    const save = vi.fn();
    render(
      <MixRelayProvider>
        <NameDialog {...props} enabled={false} allowEmpty onCreate={save} />
      </MixRelayProvider>,
    );
    const input = screen.getByRole('textbox', { name: 'Name' });
    submit(input);
    expect(save).not.toHaveBeenCalled();
  });
});
