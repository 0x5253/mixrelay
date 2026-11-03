import type { RuntimeGraphDocument } from '../../domain/control/graph';

/** One runtime/catalog selection owns its acknowledgements and subscriptions. */
export class StudioSession {
  private document: RuntimeGraphDocument | undefined;
  private readonly listeners = new Set<() => void>();

  constructor(
    readonly owner: object,
    document?: RuntimeGraphDocument,
  ) {
    this.document = document;
  }

  snapshot = () => this.document;

  subscribe = (listener: () => void) => {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  };

  accept = (document: RuntimeGraphDocument) => {
    this.document = document;
    for (const listener of this.listeners) listener();
  };
}
