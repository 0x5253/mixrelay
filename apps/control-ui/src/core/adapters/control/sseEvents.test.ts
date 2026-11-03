import { expect, it } from 'vitest';
import { MAX_SSE_BUFFER, SseEventParser } from './sseEvents';

const bytes = (text: string) => new TextEncoder().encode(text);

it('returns the data of complete events only, across arbitrary chunk boundaries', () => {
  const parser = new SseEventParser();
  expect(parser.push(bytes('event: telemetry\ndata: {"a"'))).toEqual([]);
  expect(parser.push(bytes(':1}\n'))).toEqual([]);
  expect(parser.push(bytes('\n: heartbeat\n\ndata: one\ndata: two\n\ndata: x'))).toEqual([
    '{"a":1}',
    'one\ntwo',
  ]);
  expect(parser.push(bytes('\n\n'))).toEqual(['x']);
});

it('keeps a multi-byte character split between chunks', () => {
  const parser = new SseEventParser();
  const encoded = bytes('data: é\n\n');
  expect(parser.push(encoded.slice(0, 7))).toEqual([]);
  expect(parser.push(encoded.slice(7))).toEqual(['é']);
});

it('refuses an incomplete event larger than the buffer limit', () => {
  const parser = new SseEventParser();
  expect(() => parser.push(new Uint8Array(MAX_SSE_BUFFER + 1))).toThrow(
    'telemetry event too large',
  );
});
