import { render } from '@testing-library/react';
import { expect, it } from 'vitest';
import { ProcessorTimingLabel } from './ProcessorTimingLabel';

it.each([
  [1_333_333, '1.33 ms'],
  [25_000, '25.00 µs'],
  [500, '500 ns'],
] as const)('formats the recorded unit for %i ns', (nanoseconds, label) => {
  const view = render(<ProcessorTimingLabel nanoseconds={nanoseconds} />);
  expect(view.getByText(label)).toBeVisible();
});

it.each([undefined, 0, -1, Number.NaN, Number.POSITIVE_INFINITY])(
  'hides missing or invalid timing %s',
  (nanoseconds) => {
    const view = render(<ProcessorTimingLabel nanoseconds={nanoseconds} />);
    expect(view.container).toBeEmptyDOMElement();
  },
);
