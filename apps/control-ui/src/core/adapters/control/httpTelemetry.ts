import type { TelemetryStreamListener } from '../../application/telemetry/TelemetryHub';
import type { Scheduler } from '../../ports/Scheduler';
import { SseEventParser } from './sseEvents';
import { TelemetryDecoder } from './telemetryCodec';

/** The stream must deliver its first complete snapshot within this time. */
export const TELEMETRY_STARTUP_MS = 5000;

async function pump(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  active: () => boolean,
  receive: (data: string) => void,
) {
  const parser = new SseEventParser();
  while (active()) {
    const chunk = await reader.read();
    if (chunk.done) throw new Error('telemetry ended');
    if (active()) for (const data of parser.push(chunk.value)) receive(data);
  }
}

/** Same-origin Vite bridge passes SSE unchanged over the engine's existing Unix socket. */
export async function openHttpTelemetry(
  origin: string,
  request: typeof fetch,
  listener: TelemetryStreamListener,
  scheduler: Scheduler,
): Promise<() => void> {
  const abort = new AbortController();
  const cancelStartup = scheduler.after(TELEMETRY_STARTUP_MS, () => abort.abort());
  try {
    const response = await request(`${origin}/api/v1/telemetry`, { signal: abort.signal });
    if (!response.ok || !response.body) throw new Error('telemetry unavailable');
    const reader = response.body.getReader();
    let active = true;
    const close = () => {
      active = false;
      abort.abort();
      void reader.cancel().catch(() => undefined);
    };
    await new Promise<void>((resolve, reject) => {
      let started = false;
      const decoder = new TelemetryDecoder();
      const receive = (data: string) => {
        const packet = decoder.sse(data);
        const engine = response.headers.get('X-MixRelay-Engine');
        if (engine && engine !== packet.engineIncarnation)
          throw new Error('telemetry engine changed');
        if (!started && !packet.state && !decoder.snapshotRejected)
          throw new Error('missing initial snapshot');
        listener.window(packet);
        started = true;
        cancelStartup();
        resolve();
      };
      void pump(reader, () => active, receive).catch((error: unknown) => {
        if (!active) return;
        close();
        console.error('Telemetry stream rejected; the meter stream ends', error);
        if (!started) reject(error);
        else listener.ended();
      });
    });
    return close;
  } catch (error) {
    abort.abort();
    throw error;
  } finally {
    cancelStartup();
  }
}
