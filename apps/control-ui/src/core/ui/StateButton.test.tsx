import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { StateButton } from './StateButton';

const states = [
  { value: 'pause', label: 'Pause', icon: 'Ⅱ' },
  { value: 'resume', label: 'Resume playback', icon: '▷' },
  { value: 'pending', label: 'Changing audio state…', icon: 'Ⅱ' },
];

it('retains one keyboard target and all sizing labels through a confirmed state change', () => {
  const click = vi.fn();
  const view = render(<StateButton states={states} value="pause" onClick={click} />);
  const button = screen.getByRole('button', { name: 'Pause' });
  button.focus();
  fireEvent.click(button);
  expect(click).toHaveBeenCalledOnce();
  view.rerender(<StateButton states={states} value="resume" tone="positive" aria-pressed />);
  expect(screen.getByRole('button', { name: 'Resume playback' })).toBe(button);
  expect(button).toHaveFocus();
  expect(button).toHaveAttribute('aria-pressed', 'true');
  expect(button.querySelectorAll('[aria-hidden="true"]')).toHaveLength(3);
  view.rerender(<StateButton states={states} value="pending" aria-busy disabled />);
  expect(screen.getByRole('button', { name: 'Changing audio state…' })).toBeDisabled();
});

it('accepts an authoritative accessible label when state is unavailable', () => {
  const view = render(
    <StateButton states={states} value="unavailable" aria-label="Service unavailable" disabled />,
  );
  expect(screen.getByRole('button', { name: 'Service unavailable' })).toBeDisabled();
  view.rerender(<StateButton states={states} value="unavailable" disabled />);
  expect(screen.getByRole('button')).not.toHaveAttribute('aria-label');
});
