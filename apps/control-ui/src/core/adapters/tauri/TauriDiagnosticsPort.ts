import type { SupportReportContext } from '../../domain/diagnostics';
import type { DiagnosticsPort } from '../../ports/DiagnosticsPort';
import { type NativeInvoke, tauriInvoke } from './nativeInvoke';

export class TauriDiagnosticsPort implements DiagnosticsPort {
  constructor(private readonly invoke: NativeInvoke = tauriInvoke) {}

  async saveReport(context: SupportReportContext): Promise<void> {
    await this.invoke('diagnostics_save_report', { context });
  }
}
