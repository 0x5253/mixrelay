// PipeWire 1.0.x streams raw PCM automatically for the '-' filename; newer
// pw-cat versions require --raw. Detect the advertised option rather than
// assuming all supported distributions ship the same CLI version.
export function rawPcmArgs(help) {
  const raw = /(?:^|\s)--raw(?=[\s,=]|$)/u.test(help) ? ["--raw"] : [];
  return [...raw, "--rate", "48000", "--channels", "2", "--format", "f32", "--channel-map", "stereo"];
}

// Registry changes may arrive while pw-dump is still enumerating. Even a
// non-monitor invocation can emit multiple JSON arrays; merge objects by id.
export function parsePwDump(text) {
  const objects = new Map();
  let depth = 0, quoted = false, escaped = false, start = -1, sawBatch = false;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (quoted) {
      if (escaped) escaped = false;
      else if (c === "\\") escaped = true;
      else if (c === '"') quoted = false;
    } else if (c === '"') quoted = true;
    else if (c === "[" || c === "{") { if (depth++ === 0) start = i; }
    else if (c === "]" || c === "}") {
      if (--depth < 0) throw new Error("Invalid PipeWire JSON stream");
      if (depth === 0) {
        const batch = JSON.parse(text.slice(start, i + 1));
        if (!Array.isArray(batch)) throw new Error("Expected PipeWire object array");
        sawBatch = true;
        for (const object of batch) {
          if (!Number.isSafeInteger(object.id)) throw new Error("Invalid PipeWire object identity");
          if (object.info === null) objects.delete(object.id);
          else objects.set(object.id, merge(objects.get(object.id) ?? {}, object));
        }
        start = -1;
      }
    } else if (depth === 0 && !/\s/u.test(c)) throw new Error("Invalid PipeWire JSON stream");
  }
  if (!sawBatch || depth !== 0 || quoted || start !== -1) throw new Error("Incomplete PipeWire JSON stream");
  return [...objects.values()];
}
function merge(old, update) {
  if (!old || !update || Array.isArray(old) || Array.isArray(update) ||
      typeof old !== "object" || typeof update !== "object") return update;
  const result = { ...old };
  for (const key of Object.keys(update)) Object.defineProperty(result, key,
    { value: merge(old[key], update[key]), enumerable: true, writable: true, configurable: true });
  return result;
}

// Stream nodes often publish only client.id; the owning PID lives on that
// client, not the node. A missing PID is unknown, not proof of cleanup.
export function nodesOwnedByPid(objects, pid) {
  if (!Number.isSafeInteger(Number(pid)) || Number(pid) <= 0) throw new Error('Invalid owner PID');
  const clients = new Map(objects.filter(o => o.type === 'PipeWire:Interface:Client')
    .map(o => [String(o.id), o.info?.props?.['application.process.id']]));
  return objects.filter(o => o.type === 'PipeWire:Interface:Node' &&
    String(o.info?.props?.['application.process.id'] ?? clients.get(String(o.info?.props?.['client.id']))) === String(pid));
}
