// Rewrites .icns files with their chunks in type order. The Tauri CLI writes the same
// chunks in a varying order, so every icon build would otherwise change tracked files.
//   node branding/canonical-icns.mjs <file.icns>...
import fs from 'node:fs';
import { fileURLToPath } from 'node:url';

export function canonicalIcns(file) {
  const data = fs.readFileSync(file);
  if (data.toString('latin1', 0, 4) !== 'icns' || data.readUInt32BE(4) !== data.length) {
    throw new Error(`${file} is not an icns file`);
  }
  const chunks = [];
  for (let offset = 8; offset < data.length;) {
    const length = data.readUInt32BE(offset + 4);
    if (length < 8 || offset + length > data.length) throw new Error(`${file} has a truncated chunk`);
    chunks.push({ type: data.toString('latin1', offset, offset + 4), bytes: data.subarray(offset, offset + length) });
    offset += length;
  }
  chunks.sort((a, b) => (a.type < b.type ? -1 : a.type > b.type ? 1 : Buffer.compare(a.bytes, b.bytes)));
  const result = Buffer.concat([data.subarray(0, 8), ...chunks.map(chunk => chunk.bytes)]);
  if (!result.equals(data)) fs.writeFileSync(file, result);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  for (const file of process.argv.slice(2)) canonicalIcns(file);
}
