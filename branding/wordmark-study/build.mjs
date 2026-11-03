import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { brandColors as c } from '../../apps/control-ui/src/core/theme/tokens.ts';

const root = path.dirname(fileURLToPath(import.meta.url));
const symbol = JSON.parse(fs.readFileSync(path.join(root, '../identity/source/symbol.json')));
const previous = JSON.parse(fs.readFileSync(path.join(root, '../identity/source/wordmark.json')));
const candidates = [
  { id: '01-compact', name: 'Compact', note: 'Mixed case / firm weight / optical spacing' },
  { id: '02-instrument', name: 'Instrument', note: 'Uppercase / open rhythm / straight construction' },
  { id: '03-lowercase', name: 'Lowercase', note: 'Quiet rhythm / geometric curves / softer voice' },
].map(item => ({ ...item, word: JSON.parse(fs.readFileSync(path.join(root, item.id + '.json'))) }));
const n = x => +x.toFixed(5);
const esc = x => x.replaceAll('&', '&amp;').replaceAll('<', '&lt;');
const svg = (body, width, height, title) => `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img"><title>${esc(title)}</title>${body}</svg>\n`;
const write = (file, value) => fs.writeFileSync(path.join(root, file), value);
const rect = (x, y, w, h, fill) => `<rect x="${x}" y="${y}" width="${w}" height="${h}" fill="${fill}"/>`;
const type = (x, y, content, size = 14, color = c.muted, weight = 400) => `<text x="${x}" y="${y}" font-family="Helvetica Neue,Arial,sans-serif" font-weight="${weight}" font-size="${size}" fill="${color}">${esc(content)}</text>`;
const mark = (x, y, size, color, outline = false) => `<path d="${symbol.path}" transform="translate(${n(x - 10 * size / 76)} ${n(y - 10 * size / 76)}) scale(${n(size / 76)})" ${outline ? `fill="none" stroke="${color}" stroke-width="2" stroke-linejoin="round"` : `fill="${color}"`}/>`;
const wordWidth = (w, height) => (w.bounds[2] - w.bounds[0]) * height / w.alignmentHeight;
const wordmark = (w, x, y, height, color) => {
  const scale = height / w.alignmentHeight;
  return `<path d="${w.path}" transform="translate(${n(x - w.bounds[0] * scale)} ${n(y + height)}) scale(${n(scale)})" fill="${color}"/>`;
};
const lockup = (w, x, y, height, symbolColor, typeColor = symbolColor, outline = false) =>
  mark(x, y, height, symbolColor, outline) + wordmark(w, x + height * 1.4, y + height * .2, height * .6, typeColor);

for (const item of candidates) {
  const width = Math.ceil(32 + 64 * 1.4 + wordWidth(item.word, 38.4));
  for (const [tone, symbolColor, typeColor, background, outline] of [
    ['primary', c.primary, c.ink, null, false],
    ['mono', c.ink, c.ink, null, false],
    ['inverse', c.inverse, c.inverse, c.dark, false],
    ['outline', c.primary, c.ink, null, true],
  ]) {
    write(`${item.id}-${tone}.svg`, svg((background ? rect(0, 0, width, 96, background) : '') +
      lockup(item.word, 16, 16, 64, symbolColor, typeColor, outline), width, 96, `MixRelay wordmark study ${item.name} ${tone}`));
  }
  const wordHeight = (item.word.bounds[3] + item.word.alignmentHeight) * 64 / item.word.alignmentHeight;
  write(`${item.id}-wordmark.svg`, svg(wordmark(item.word, 16, 16, 64, c.ink),
    Math.ceil(wordWidth(item.word, 64) + 32), Math.ceil(wordHeight + 32), `MixRelay ${item.name} outlined wordmark`));
}

let board = rect(0, 0, 1200, 1512, c.paper);
board += type(64, 63, 'WITHDRAWN / INITIAL FONT COMPARISON', 15, c.primary, 500);
board += type(64, 116, 'Same symbol. Three typographic directions.', 30, c.ink, 500);
board += type(64, 147, '01 compact  /  02 instrument  /  03 lowercase', 15);
for (let i = 0; i < candidates.length; i++) {
  const item = candidates[i];
  const top = 200 + i * 370;
  board += `<path d="M64 ${top}H1136" stroke="${c.muted}" stroke-opacity=".2"/>`;
  board += type(64, top + 38, `${String(i+1).padStart(2, '0')}  ${item.name.toUpperCase()}`, 15, c.ink, 500);
  board += type(64, top + 63, item.note, 13);
  board += lockup(item.word, 64, top + 102, 104, c.primary, c.ink);
  board += type(64, top + 247, 'WORDMARK DETAIL', 11);
  board += wordmark(item.word, 64, top + 270, 54, c.ink);
  board += rect(848, top + 32, 288, 300, c.dark);
  board += type(872, top + 62, 'INVERSE', 11, c.inverse);
  board += lockup(item.word, 872, top + 86, 38, c.inverse);
  board += type(872, top + 165, '24 PX SYMBOL', 10, c.inverse);
  board += lockup(item.word, 872, top + 180, 24, c.inverse);
  board += type(872, top + 248, '16 PX SYMBOL / 10 PX TYPE', 10, c.inverse);
  board += mark(872, top + 268, 16, c.inverse);
  board += wordmark(item.word, 896, top + 271, 10, c.inverse);
}
board += `<path d="M64 1340H1136" stroke="${c.muted}" stroke-opacity=".2"/>`;
board += type(64, 1377, 'CURRENT WORDMARK / REFERENCE', 11);
board += `<path d="${previous.path}" transform="translate(64 1416) scale(${n(22 / previous.capHeight)})" fill="${c.muted}"/>`;
board += type(64, 1468, 'WITHDRAWN: insufficient basis for selecting a wordmark. See typography-brief.md.', 12, c.primary);
write('comparison.svg', svg(board, 1200, 1512, 'MixRelay three wordmark directions'));
write('manifest.json', JSON.stringify({ status: 'withdrawn / insufficient typography rationale', recommendation: null, nextBasis: 'typography-brief.md', symbol: '../identity/source/symbol.json',
  typeHeightToSymbol: .6, gapToSymbol: .4,
  candidates: candidates.map(({word, ...item}) => ({...item, text: word.text, font: word.font,
    tracking: word.tracking, pairAdjustments: word.pairAdjustments})), colors: c }, null, 2) + '\n');
console.log('Generated 15 outlined SVG assets and comparison sheet; production assets unchanged.');
