import assert from 'node:assert/strict';

export function parseNativeTelemetry(text) {
  const rows = text.split('\n');
  const devices = {}, timing = {};
  for (const line of rows.slice(1, -1)) {
    if (!line.includes('native PCM queues') && !line.includes('PCM callback execution timing')) continue;
    const fields = Object.fromEntries([...line.matchAll(/(\w+)=([^\s]+)/g)].map(([, key, value]) => [key,
      value === 'true' ? true : value === 'false' ? false : Number.isFinite(Number(value)) ? Number(value) : value]));
    if (fields.owner === undefined) continue;
    const destination = line.includes('native PCM queues') ? devices : timing;
    destination[fields.owner] = { at: line.split(' ')[0], ...fields };
  }
  return { devices, timing };
}

export function continuityFailures(before, last, observedAt) {
  const failures = [];
    for (const key of ['underruns', 'overruns']) {
      if (last.status[key] !== before.status[key]) failures.push(`${last.at}: ${key} ${before.status[key]} -> ${last.status[key]}`);
    }
    if (last.status.audioSession?.discontinuities !== before.status.audioSession?.discontinuities
      || JSON.stringify(last.status.audioSession?.format) !== JSON.stringify(before.status.audioSession?.format)
      || last.status.audioSession?.issue || last.status.audioSession?.processingState !== 'running') failures.push(`${last.at}: session changed or unhealthy`);
    for (const [id, initial] of Object.entries(before.devices)) {
      const now = last.devices[id];
      if (!now || !Number.isFinite(Date.parse(now.at)) || observedAt - Date.parse(now.at) > 5000) { failures.push(`${last.at}: device ${id} telemetry missing/stale`); continue; }
      for (const field of ['pcm_starvations', 'overflows', 'clock_gaps', 'epoch', 'native_rate', 'graph_rate', 'enabled', 'ready']) {
        if (initial[field] !== now[field]) failures.push(`${last.at}: device ${id} ${field} ${initial[field]} -> ${now[field]}`);
      }
    }
  return failures;
}

export function referenceFailures(reference, last, afterGraphs) {
  const failures = [];
    assert(reference.type === 'result' && reference.after && reference.afterGraphs, 'Reference needs a completed observation');
    if (JSON.stringify(reference.afterGraphs) !== JSON.stringify(afterGraphs)) failures.push('Reference graph configuration/revision changed');
    for (const field of ['underruns', 'overruns']) {
      if (last.status[field] !== reference.after.status[field]) failures.push(`Reference ${field}: ${reference.after.status[field]} -> ${last.status[field]}`);
    }
    for (const field of ['discontinuities', 'format']) {
      if (JSON.stringify(last.status.audioSession?.[field]) !== JSON.stringify(reference.after.status.audioSession?.[field])) failures.push(`Reference session ${field} changed`);
    }
    if (Object.keys(reference.after.devices).sort().join(',') !== Object.keys(last.devices).sort().join(',')) failures.push('Reference native owner set changed');
    for (const [id, initial] of Object.entries(reference.after.devices)) {
      for (const field of ['pcm_starvations', 'overflows', 'clock_gaps', 'epoch', 'native_rate', 'graph_rate', 'enabled', 'ready']) {
        if (initial[field] !== last.devices[id]?.[field]) failures.push(`Reference device ${id} ${field}: ${initial[field]} -> ${last.devices[id]?.[field]}`);
      }
    }
  return failures;
}
