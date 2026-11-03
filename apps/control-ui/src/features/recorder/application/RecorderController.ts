import { hasErrorCode } from '../../../core/domain/control/errors';
import type {
  RecorderRequest,
  RecorderSettings,
  RecorderStatus,
} from '../../../core/domain/control/recorder';
import type { RecorderPort } from '../../../core/ports/control/RecorderPort';
import type { Cancel, Scheduler } from '../../../core/ports/Scheduler';
import { recorderError } from '../domain/recording';

interface RecorderSnapshot {
  status: RecorderStatus | null;
  settings: RecorderSettings | null;
  settingsError: string | null;
  busy: boolean;
  error: string | null;
}

/** One graph/node owner serializes reads and commands; disposed owners never publish late replies. */
export class RecorderController {
  private snapshot: RecorderSnapshot = {
    status: null,
    settings: null,
    settingsError: null,
    busy: false,
    error: null,
  };
  private readonly listeners = new Set<() => void>();
  private active = false;
  private generation = 0;
  private timer: Cancel | undefined;
  private tail: Promise<void> = Promise.resolve();
  private commandError: string | null = null;

  public constructor(
    private readonly port: RecorderPort,
    private readonly graphId: string,
    private readonly nodeId: string,
    private readonly scheduler: Scheduler,
  ) {}

  public readonly getSnapshot = () => this.snapshot;
  public readonly subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  public start(): void {
    if (this.active) return;
    this.generation += 1;
    this.active = true;
    this.publish({ ...this.snapshot, busy: false });
    this.refresh();
  }

  public dispose(): void {
    this.generation += 1;
    this.active = false;
    this.timer?.();
  }

  public readonly command = (request: RecorderRequest, expectedRevision: number): void => {
    if (!this.active || this.snapshot.busy) return;
    if (
      request.action === 'record' &&
      this.snapshot.status?.state !== 'paused' &&
      (!this.snapshot.settings || this.snapshot.settingsError)
    )
      return;
    this.timer?.();
    this.commandError = null;
    this.publish({ ...this.snapshot, busy: true, error: null });
    const generation = this.generation;
    this.tail = this.tail.then(async () => {
      if (!this.active || generation !== this.generation) return;
      try {
        const status = await this.port.controlRecorder(
          this.graphId,
          this.nodeId,
          request,
          expectedRevision,
        );
        if (generation !== this.generation) return;
        this.publish({ ...this.snapshot, status, busy: false, error: status.error });
      } catch (error) {
        if (generation !== this.generation) return;
        this.commandError = recorderError(error);
        this.publish({ ...this.snapshot, busy: false, error: this.commandError });
      }
      this.schedule();
    });
  };

  public readonly saveSettings = (
    change: Partial<RecorderSettings>,
    expectedRevision: number,
  ): void => {
    this.editSettings(async (settings) => ({ ...settings, ...change }), expectedRevision);
  };

  public readonly chooseFolder = (title: string, expectedRevision: number): void => {
    const choose = this.port.selectRecordingFolder?.bind(this.port);
    if (!choose) return;
    this.editSettings(async (settings) => {
      const folder = await choose({ defaultPath: settings.folder, title });
      return folder === null ? null : { ...settings, folder };
    }, expectedRevision);
  };

  private editSettings(
    select: (settings: RecorderSettings) => Promise<RecorderSettings | null>,
    expectedRevision: number,
  ): void {
    if (
      !this.active ||
      this.snapshot.busy ||
      !this.snapshot.settings ||
      this.snapshot.status?.state === 'recording' ||
      this.snapshot.status?.state === 'paused'
    )
      return;
    this.timer?.();
    this.publish({ ...this.snapshot, busy: true });
    const generation = this.generation;
    this.tail = this.tail.then(async () => {
      if (!this.owns(generation) || !this.snapshot.settings) return;
      try {
        const selected = await select(this.snapshot.settings);
        // A native picker can outlive this graph/node; never write after ownership changes.
        if (!this.owns(generation)) return;
        if (selected !== null) {
          const settings = await this.port.setRecordingSettings(
            this.graphId,
            this.nodeId,
            selected,
            expectedRevision,
          );
          if (!this.owns(generation)) return;
          this.publish({ ...this.snapshot, settings, settingsError: null });
        }
      } catch (error) {
        if (!this.owns(generation)) return;
        this.publish({ ...this.snapshot, settingsError: recorderError(error) });
      }
      this.publish({ ...this.snapshot, busy: false });
      this.schedule();
    });
  }

  private owns(generation: number): boolean {
    return this.active && generation === this.generation;
  }

  private refresh(): void {
    const generation = this.generation;
    this.tail = this.tail.then(async () => {
      if (!this.active || generation !== this.generation || this.snapshot.busy) return;
      const [status, settings] = await Promise.allSettled([
        this.port.recorderStatus(this.graphId, this.nodeId),
        this.port.recordingSettings(this.graphId, this.nodeId),
      ]);
      if (!this.owns(generation)) return;
      this.publish({
        ...this.snapshot,
        settings: settings.status === 'fulfilled' ? settings.value : this.snapshot.settings,
        settingsError: settings.status === 'fulfilled' ? null : recorderError(settings.reason),
      });
      if (status.status === 'fulfilled') {
        this.publish({
          ...this.snapshot,
          status: status.value,
          error: this.commandError ?? status.value.error,
        });
      } else {
        this.readFailed(status.reason);
      }
      this.schedule();
    });
  }

  private readFailed(error: unknown): void {
    // A contested read is not a recorder failure; commands still expose this rejection.
    if (hasErrorCode(error, 'graph_exchange_busy')) return;
    const removed = hasErrorCode(error, 'node_not_found');
    this.publish({
      ...this.snapshot,
      status: removed ? null : this.snapshot.status,
      settings: removed ? null : this.snapshot.settings,
      error: this.commandError ?? recorderError(error),
    });
  }

  private schedule(): void {
    this.timer?.();
    if (this.active)
      this.timer = this.scheduler.after(
        this.snapshot.status?.state === 'recording' ? 100 : 1000,
        () => this.refresh(),
      );
  }

  private publish(snapshot: RecorderSnapshot): void {
    if (!this.active) return;
    this.snapshot = snapshot;
    for (const listener of this.listeners) listener();
  }
}
