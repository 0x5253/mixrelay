import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { brandColors } from '../apps/control-ui/src/core/theme/tokens.ts';

const root = path.dirname(fileURLToPath(import.meta.url));
const source = JSON.parse(fs.readFileSync(path.join(root, 'identity/source/wordmark.json'), 'utf8'));
const symbol = JSON.parse(fs.readFileSync(path.join(root, 'identity/source/symbol.json'), 'utf8'));
const output = path.join(root, 'identity/wordmark');
fs.mkdirSync(output, { recursive: true });
const n = value => +value.toFixed(5);
const H = source.capHeight;
const pad = H * source.clearSpaceRatio;
const wordOffset = H * (1 + source.gapRatio);
const assets = [];
const master = path.resolve(root, 'identity/source', source.illustratorMaster);
const masterHash = crypto.createHash('sha256').update(fs.readFileSync(master)).digest('hex');
if (masterHash !== source.illustratorSha256) throw new Error('Frozen Illustrator master changed; review before replacing approved assets.');
if (source.status !== 'frozen' || source.glyphs.map(g => g.character).join('') !== 'MixRelay') throw new Error('Unexpected wordmark source.');

export function artwork({ kind = 'lockup', outline = false, symbolColor, wordColor }) {
  const paint = color => outline
    ? `fill="none" stroke="${color}" stroke-width="${source.outlineWidth}" stroke-linejoin="round"`
    : `fill="${color}" fill-rule="nonzero"`;
  const mark = kind === 'lockup'
    ? `<path id="symbol" d="${symbol.path}" transform="translate(-10 -10)" ${paint(symbolColor)}/>` : '';
  const letters = source.glyphs.map((g, i) => `<path id="letter-${i}-${g.character}" d="${g.path}" ${paint(wordColor)}/>`).join('');
  return mark + `<g id="wordmark" transform="translate(${kind === 'lockup' ? n(wordOffset) : 0} 0)">${letters}</g>`;
}

function emit(name, kind, outline, symbolColor, wordColor, theme) {
  const inkWidth = source.wordBounds[2] + (kind === 'lockup' ? wordOffset : 0);
  const width = n(inkWidth + pad * 2);
  const height = n(source.wordBounds[3] + pad * 2);
  const body = artwork({ kind, outline, symbolColor, wordColor });
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="${n(-pad)} ${n(-pad)} ${width} ${height}" role="img"><title>MixRelay Optical ${name}</title>${body}</svg>\n`;
  fs.writeFileSync(path.join(output, name), svg);
  assets.push({ file: name, kind, theme, treatment: outline ? 'outline' : 'filled', symbolColor, wordColor, width, height, sha256: crypto.createHash('sha256').update(svg).digest('hex') });
}

for (const outline of [false, true]) {
  const suffix = outline ? '-outline' : '';
  for (const [theme, symbolColor, wordColor] of [
    ['light', brandColors.primary, brandColors.ink],
    ['dark', brandColors.primary, brandColors.inverse],
    ['mono-light', brandColors.ink, brandColors.ink],
    ['inverse-dark', brandColors.inverse, brandColors.inverse],
  ]) emit(`mixrelay-lockup-${theme}${suffix}.svg`, 'lockup', outline, symbolColor, wordColor, theme);
  for (const [role, wordColor] of [['ink', brandColors.ink], ['inverse', brandColors.inverse], ['red', brandColors.primary]]) {
    emit(`mixrelay-wordmark-${role}${suffix}.svg`, 'wordmark', outline, undefined, wordColor, role === 'inverse' ? 'dark' : 'light');
  }
}

fs.writeFileSync(path.join(output, 'manifest.json'), JSON.stringify({
  version: source.version, status: source.status, approvedAt: source.approvedAt,
  geometry: '../source/wordmark.json', symbol: '../source/symbol.json',
  illustrator: '../../wordmark-study/plex/mixrelay-plex-wordmark.ai',
  illustratorSha256: masterHash,
  colors: 'apps/control-ui/src/core/theme/tokens.ts#brandColors', colorValues: brandColors,
  capToSymbol: 1, gapToSymbol: source.gapRatio, clearSpaceToSymbol: source.clearSpaceRatio,
  outlineWidth: source.outlineWidth, minimumCapHeight: { filled: 16, outline: 64 },
  minimumSizeStatus: 'Conservative asset guidance; installed-app rendering not verified.',
  productionScreensUpdated: false, assets,
}, null, 2) + '\n');
console.log(`Frozen Optical: ${assets.length} SVG assets generated from approved paths and Core colors.`);
