import { probeRangeHash } from './probe-signal.mjs';

// Compare the sum only when the retained writes establish one reference signal
// and digital silence from every other writer in the selected signal window.
export function analyseNativeReference(trace, { origin, start, end, channels }) {
  const groups = new Map();
  for (const row of trace.rows.filter(row => row[1] === 1)) {
    const client = row[8] ?? null;
    if (!groups.has(client)) groups.set(client, []);
    if (row[2] < end && row[2] + row[3] > start) groups.get(client).push(row);
  }
  const full = row => row[2] >= start && row[2] + row[3] <= end;
  const hashes = new Map();
  const expectedHash = row => {
    const key = `${row[2]}:${row[3]}`;
    if (!hashes.has(key)) hashes.set(key, probeRangeHash(row[2] - origin, row[3], channels));
    return hashes.get(key);
  };
  const clients = [...groups].map(([client, rows]) => {
    const complete = rows.length > 0 && rows.every(row => row[4] === 1 && row[5] === row[3]);
    const selected = rows.filter(full);
    return { client, selected,
      silent: complete && rows.every(row => row[6] === row[3]),
      reference: complete && selected.length > 0 && selected.every(row => row[7] === expectedHash(row)),
    };
  });
  const signals = clients.filter(client => client.reference);
  const source = signals.length === 1 ? signals[0] : null;
  const knownMix = source && clients.every(client => client === source || client.silent);
  const comparisonStatus = clients.length === 0 ? 'no_retained_output'
    : clients.length === 1 ? 'single_output'
    : knownMix ? 'signal_plus_silence' : 'inapplicable_multiple_client_mix';
  const comparable = clients.length > 0 && comparisonStatus !== 'inapplicable_multiple_client_mix';
  const writes = clients.length === 1 ? clients[0].selected : knownMix ? source.selected : [];
  const captures = comparable ? trace.rows.filter(row => row[1] === 0 && full(row)) : [];
  const differences = rows => rows.filter(row => row[7] !== expectedHash(row)).map(row => ({
    sequence: row[0], frame: row[2], frames: row[3], observed: row[7], expected: expectedHash(row),
  }));
  return {
    nativeWrites: { comparisonStatus, comparedRows: writes.length,
      writeClients: [...groups.keys()], sourceClient: source?.client ?? null, differences: differences(writes) },
    nativeCapture: { comparisonStatus, comparedRows: captures.length, differences: differences(captures) },
  };
}
