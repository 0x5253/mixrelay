import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { DisclosureList } from './DisclosureList';

it('opens each row in place from its heading button and keeps others independent', async () => {
  const user = userEvent.setup();
  render(
    <MixRelayProvider>
      <DisclosureList
        items={[
          { id: 'a', title: 'First issue', content: 'First detail' },
          { id: 'b', title: 'Second issue', content: 'Second detail' },
        ]}
      />
    </MixRelayProvider>,
  );
  const first = screen.getByRole('button', { name: 'First issue' });
  const second = screen.getByRole('button', { name: 'Second issue' });
  expect(screen.getByRole('heading', { level: 3, name: 'First issue' })).toContainElement(first);
  expect(screen.getByText('First detail')).not.toBeVisible();
  await user.click(first);
  first.focus();
  await user.tab();
  expect(second).toHaveFocus();
  await user.keyboard('{Enter}');
  expect(first).toHaveAttribute('aria-expanded', 'true');
  expect(second).toHaveAttribute('aria-expanded', 'true');
  expect(screen.getByRole('group', { name: 'First issue' })).toHaveTextContent('First detail');
  await user.click(first);
  expect(screen.getByText('First detail')).not.toBeVisible();
  expect(screen.getByText('Second detail')).toBeVisible();
});
