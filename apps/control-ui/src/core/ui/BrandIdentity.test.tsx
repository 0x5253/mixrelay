import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { BrandIdentity, ProductHeading } from './BrandIdentity';

it('shows only the approved symbol while preserving the heading name without a keyboard target', () => {
  render(<ProductHeading />);
  expect(screen.getByRole('heading', { name: 'MixRelay', level: 1 })).toBeVisible();
  const logo = screen.getByRole('img', { name: 'MixRelay' });
  expect(logo).not.toHaveAttribute('tabindex');
  expect(logo).toHaveAttribute('focusable', 'false');
  expect(logo).toHaveAttribute('width', '16');
  expect(logo).toHaveAttribute('height', '16');
  expect(logo.querySelectorAll('path')).toHaveLength(1);
});

it('keeps outline artwork at display size and supplies the same product name in inverse use', () => {
  const { rerender } = render(<BrandIdentity />);
  expect(screen.getByRole('img', { name: 'MixRelay' })).toHaveAttribute('data-tone', 'primary');
  rerender(<BrandIdentity size="standard" tone="neutral" />);
  expect(screen.getByRole('img', { name: 'MixRelay' })).toHaveAttribute('height', '24');
  rerender(<BrandIdentity size="compact" tone="inverse" outline />);
  expect(screen.getByRole('img', { name: 'MixRelay' })).toHaveAttribute('data-size', 'display');
  expect(screen.getByRole('img', { name: 'MixRelay' })).toHaveAttribute('data-tone', 'inverse');
  rerender(<BrandIdentity outline />);
  expect(screen.getByRole('img', { name: 'MixRelay' })).toHaveAttribute('data-outline', 'true');
});
