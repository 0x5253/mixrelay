import { expect, it, vi } from 'vitest';
import { scanFailureCounts } from '../../domain/diagnostics';
import { TauriDiagnosticsPort } from './TauriDiagnosticsPort';

it('sends only codes and scan counts to the native report command', async () => {
  const invoke = vi.fn().mockResolvedValue(undefined);
  const failure = { format: 'vst3' as const, stage: 'probe' as const, message: '/Users/name' };
  const context = {
    problems: ['graph_apply_timeout'],
    processorScan: scanFailureCounts([
      failure,
      failure,
      { ...failure, pluginName: 'Other' },
      { format: 'clap', stage: 'scan', message: 'denied' },
    ]),
  };
  await new TauriDiagnosticsPort(invoke).saveReport(context);
  expect(invoke.mock.calls).toEqual([
    [
      'diagnostics_save_report',
      {
        context: {
          problems: ['graph_apply_timeout'],
          processorScan: [
            { format: 'vst3', stage: 'probe', count: 2 },
            { format: 'clap', stage: 'scan', count: 1 },
          ],
        },
      },
    ],
  ]);
  expect(JSON.stringify(context)).not.toMatch(/Users|Other|denied/u);
});
