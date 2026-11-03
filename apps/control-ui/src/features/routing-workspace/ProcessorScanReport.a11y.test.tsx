import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import axe from 'axe-core';
import { expect, it, vi } from 'vitest';
import { i18n } from '../../core/i18n';
import { MixRelayProvider } from '../../core/theme/MixRelayProvider';
import { ProcessorScanReport } from './ProcessorScanReport';

it('keeps the portalled scan report accessible before and after technical disclosure', async () => {
  await i18n.changeLanguage('ko-KR');
  render(
    <MixRelayProvider>
      <ProcessorScanReport
        opened
        onClose={vi.fn()}
        catalog={{
          processors: [],
          diagnostics: [
            {
              format: 'vst3',
              stage: 'probe',
              pluginName: 'Example Effect',
              message: 'Validation failed',
            },
          ],
          busy: false,
          failure: null,
          refresh: vi.fn(),
          retry: vi.fn(),
        }}
      />
    </MixRelayProvider>,
  );
  const rules = { 'color-contrast': { enabled: false }, region: { enabled: false } };
  expect((await axe.run(document.body, { rules })).violations).toEqual([]);
  await userEvent.setup().click(screen.getByRole('button', { name: /Example Effect/ }));
  expect((await axe.run(document.body, { rules })).violations).toEqual([]);
});
