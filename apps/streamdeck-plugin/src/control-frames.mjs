const RESPONSE_LIMIT = 4 * 1024 * 1024;

export function encodeCommand(command) {
  return `${JSON.stringify({ id: 1, command })}\n`;
}

function decode(frame) {
  const response = JSON.parse(new TextDecoder().decode(frame));
  if (response.id !== 1 || !Number.isInteger(response.status)) throw new Error('Invalid response');
  if (response.status === 102) return { pending: true };
  if (response.status < 200 || response.status >= 300)
    throw new Error(response.body?.code ?? 'Control rejected');
  return { pending: false, body: response.body };
}

/** Bounds each response frame before decoding, including fragments without a delimiter. */
export class ControlFrames {
  #parts = [];
  #size = 0;

  *accept(chunk) {
    let offset = 0;
    while (offset < chunk.length) {
      const newline = chunk.indexOf(10, offset);
      const end = newline < 0 ? chunk.length : newline;
      const segment = chunk.subarray(offset, end);
      this.#size += segment.length;
      if (this.#size > RESPONSE_LIMIT) throw new Error('Response too large');
      if (segment.length > 0) this.#parts.push(segment);
      if (newline < 0) return;
      const frame = new Uint8Array(this.#size);
      let written = 0;
      for (const part of this.#parts) {
        frame.set(part, written);
        written += part.length;
      }
      this.#parts = [];
      this.#size = 0;
      offset = newline + 1;
      yield decode(frame);
    }
  }
}
