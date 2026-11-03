import { StorybookMockControlPort } from '../core/adapters/fake/StorybookMockControlPort';
import '../test-support/integration';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { processorReportState } from '../../.storybook/fixtures/processorReport';
import { I18nProvider, i18n } from '../core/i18n';
import { MixRelayProvider } from '../core/theme/MixRelayProvider';
import { advance, eventually, find, finishFakeTimerTest } from '../test-support/fakeTimerQueries';
import { StudioScreen } from './StudioScreen';

beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(228);
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(120);
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(private callback: ResizeObserverCallback) {}
      observe(target: Element) {
        queueMicrotask(() =>
          this.callback(
            [
              {
                target,
                borderBoxSize: [],
                contentBoxSize: [],
                devicePixelContentBoxSize: [],
                contentRect: new DOMRect(0, 0, 228, 120),
              },
            ],
            this,
          ),
        );
      }
      unobserve() {}
      disconnect() {}
    },
  );
});

afterEach(() => finishFakeTimerTest(() => act(async () => vi.runOnlyPendingTimersAsync())));

it('shares one scan session between the bottom-right diagnostics and processor menu without stopping audio', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const state = processorReportState();
  state.status.enginePower = { enabled: true, transitioning: false };
  const port = new StorybookMockControlPort(state);
  const controlPort = Object.assign(port, { setEnginePower: vi.fn() });
  const lookup = vi.spyOn(port, 'getProcessors');
  const scan = vi.spyOn(port, 'rescanProcessors').mockImplementation(async () => {
    if (state.processors) state.processors.diagnostics = [];
    return port.getProcessors();
  });
  const apply = vi.spyOn(port, 'applyGraph');
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const view = render(
    <MixRelayProvider>
      <I18nProvider>
        <StudioScreen controlPort={controlPort} />
      </I18nProvider>
    </MixRelayProvider>,
  );
  const diagnostics = await find('button', { name: 'Diagnostics · 19' });
  await advance();
  expect(lookup).toHaveBeenCalledTimes(1);
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  await user.click(diagnostics);
  const dialog = await find('dialog', { name: 'Diagnostics' });
  expect(dialog).toHaveTextContent('Unavailable Effect 19');
  expect(scan).not.toHaveBeenCalled();
  expect(controlPort.setEnginePower).not.toHaveBeenCalled();
  await user.click(within(dialog).getByRole('button', { name: 'Rescan audio effects' }));
  await eventually(() =>
    expect(dialog).toHaveTextContent('Effect search completed without issues.'),
  );
  expect(scan).toHaveBeenCalledExactlyOnceWith([]);
  await user.keyboard('{Escape}');
  await eventually(() => expect(diagnostics).toHaveFocus());
  expect(diagnostics).toHaveAccessibleName('Diagnostics');
  await user.click(screen.getByRole('button', { name: 'Add node' }));
  const processor = await find('menuitem', { name: 'Processor' });
  processor.focus();
  await user.keyboard('{ArrowRight}');
  await user.click(await find('menuitem', { name: 'View search report…' }));
  const menuReport = await find('dialog', { name: 'Effect search report' });
  expect(menuReport).toHaveTextContent('Effect search completed without issues.');
  expect(within(menuReport).queryByText('Unavailable Effect 19')).not.toBeInTheDocument();
  expect(lookup).toHaveBeenCalledTimes(2);
  expect(apply).not.toHaveBeenCalled();
  expect(controlPort.setEnginePower).not.toHaveBeenCalled();
  view.unmount();
});

it('moves workspace failures into diagnostics and keeps focus there after recovery', async () => {
  vi.useFakeTimers();
  await i18n.changeLanguage('en');
  const state = processorReportState();
  const port = new StorybookMockControlPort(state);
  vi.spyOn(port, 'getGraphLayout').mockRejectedValueOnce(new Error('unknown acknowledgement'));
  const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTimeAsync });
  const view = render(
    <MixRelayProvider>
      <I18nProvider>
        <StudioScreen controlPort={port} />
      </I18nProvider>
    </MixRelayProvider>,
  );
  const diagnostics = await find('button', { name: 'Diagnostics · 20' });
  const workspace = screen.getByRole('region', { name: 'Routing workspace' });
  expect(within(workspace).queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  expect(screen.getByText('Diagnostics: items to check (20)')).toHaveAttribute('role', 'status');
  expect(
    screen.getByRole('img', { name: 'Diagnostics: current problems (1)' }),
  ).toBeInTheDocument();
  await user.click(diagnostics);
  const dialog = await find('dialog', { name: 'Diagnostics' });
  const problems = within(dialog).getByRole('region', { name: 'Current problems' });
  await eventually(() =>
    expect(within(problems).getByRole('button', { name: 'Retry' })).toBeInTheDocument(),
  );
  const retry = within(problems).getByRole('button', { name: 'Retry' });
  await user.click(retry);
  await eventually(() => expect(within(dialog).queryByText('Current problems')).toBeNull());
  await eventually(() => expect(dialog.contains(document.activeElement)).toBe(true));
  await user.keyboard('{Escape}');
  await eventually(() => expect(diagnostics).toHaveFocus());
  expect(diagnostics).toHaveAccessibleName('Diagnostics · 19');
  expect(screen.getByText('Diagnostics: items to check (19)')).toHaveAttribute('role', 'status');
  expect(screen.queryByRole('img', { name: /current problems/u })).not.toBeInTheDocument();
  view.unmount();
});
