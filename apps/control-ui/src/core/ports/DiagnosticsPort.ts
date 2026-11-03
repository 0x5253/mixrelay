import type { SupportReportContext } from '../domain/diagnostics';

/** The native side opens the save dialog, collects runtime facts and writes the file. */
export interface DiagnosticsPort {
  saveReport(context: SupportReportContext): Promise<void>;
}
