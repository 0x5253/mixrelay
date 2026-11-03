import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MixRelayProvider } from '../theme/MixRelayProvider';
import { ActionButton } from './ActionButton';
import { ContentSection } from './ContentSection';
import { FormActions } from './FormActions';
import { Inline } from './Inline';
import { InlineNotice } from './InlineNotice';
import { Stack } from './Stack';
import { Text } from './Text';
import { TextField } from './TextField';

it('keeps the native form order and submit behavior without adding focusable layout wrappers', () => {
  const submit = vi.fn();
  const cancel = vi.fn();
  const { container } = render(
    <MixRelayProvider>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          submit();
        }}
      >
        <Stack>
          <TextField label="Device" defaultValue="MixRelay IO" />
        </Stack>
        <FormActions>
          <ActionButton onClick={cancel}>Cancel</ActionButton>
          <ActionButton type="submit">Save</ActionButton>
        </FormActions>
      </form>
    </MixRelayProvider>,
  );
  const controls = container.querySelectorAll('input, button, [tabindex]');
  expect(Array.from(controls).map((element) => element.tagName)).toEqual([
    'INPUT',
    'BUTTON',
    'BUTTON',
  ]);
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(cancel).toHaveBeenCalledOnce();
  expect(submit).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Save' }));
  expect(submit).toHaveBeenCalledOnce();
});

it('announces failed operations urgently, keeps the retry control accessible, and retains normal status separately', () => {
  const retry = vi.fn();
  render(
    <MixRelayProvider>
      <Stack gap="section">
        <InlineNotice>Device ready</InlineNotice>
        <InlineNotice tone="warning" title="Paused">
          Recording is paused
        </InlineNotice>
        <InlineNotice
          tone="critical"
          title="Failed"
          actions={<ActionButton onClick={retry}>Retry</ActionButton>}
        >
          The confirmed settings are retained
        </InlineNotice>
        <Inline align="baseline" justify="between" wrap={false}>
          <span>Device</span>
          <span>2 CH</span>
        </Inline>
      </Stack>
    </MixRelayProvider>,
  );
  const alert = screen.getByRole('alert');
  expect(alert).toHaveTextContent('Failed');
  expect(alert).toHaveTextContent('The confirmed settings are retained');
  expect(screen.getAllByRole('status')).toHaveLength(2);
  fireEvent.click(within(alert).getByRole('button', { name: 'Retry' }));
  expect(retry).toHaveBeenCalledOnce();
});

it('labels each content group with its own heading and keeps typography roles out of ARIA roles', () => {
  render(
    <Stack gap="wide">
      <ContentSection title="MixRelay" divider="after">
        <Text kind="supporting" tone="muted">
          Version 0.1.3
        </Text>
      </ContentSection>
      <ContentSection title="MixRelay IO" level={3} divider="before">
        <Text as="span" kind="caption">
          48 kHz
        </Text>
      </ContentSection>
    </Stack>,
  );
  const driver = screen.getByRole('region', { name: 'MixRelay' });
  const device = screen.getByRole('region', { name: 'MixRelay IO' });
  expect(within(driver).getByRole('heading', { level: 2 })).toHaveTextContent('MixRelay');
  expect(within(device).getByRole('heading', { level: 3 })).toHaveTextContent('MixRelay IO');
  expect(within(driver).getByText('Version 0.1.3')).not.toHaveAttribute('role');
  expect(within(device).getByText('48 kHz').tagName).toBe('SPAN');
});
