/** Larger buffered input means the stream is not telemetry; it is refused, not grown. */
export const MAX_SSE_BUFFER = 4 * 1024 * 1024;

/** Splits a server-sent event stream into the data of each complete event. Chunks may end
 * anywhere, including inside a multi-byte character or between the two newlines. */
export class SseEventParser {
  private buffer = '';
  private decoder = new TextDecoder();

  push(chunk: Uint8Array): string[] {
    this.buffer += this.decoder.decode(chunk, { stream: true });
    if (this.buffer.length > MAX_SSE_BUFFER) throw new Error('telemetry event too large');
    const events: string[] = [];
    for (let end = this.buffer.indexOf('\n\n'); end !== -1; end = this.buffer.indexOf('\n\n')) {
      const event = this.buffer.slice(0, end);
      this.buffer = this.buffer.slice(end + 2);
      const data = event
        .split('\n')
        .filter((line) => line.startsWith('data:'))
        .map((line) => line.slice(5).trimStart())
        .join('\n');
      if (data) events.push(data);
    }
    return events;
  }
}
