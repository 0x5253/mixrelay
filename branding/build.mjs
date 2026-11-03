import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { brandColors as c } from '../apps/control-ui/src/core/theme/tokens.ts';
import './build-wordmark.mjs';

// Geometry is selected by the user; color ownership remains in Core tokens.
const root = path.dirname(fileURLToPath(import.meta.url));
const identity = path.join(root, 'identity');
const symbol = JSON.parse(fs.readFileSync(path.join(identity, 'source/symbol.json'), 'utf8'));
const n = value => +value.toFixed(5);
const xml = value => value.replaceAll('&', '&amp;').replaceAll('<', '&lt;');
const svg = (body, width, height, title, viewBox = `0 0 ${width} ${height}`) =>
  `<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="${viewBox}" role="img"><title>${xml(title)}</title>${body}</svg>\n`;
const write = (name, value) => {
  const destination = path.join(root, name);
  fs.mkdirSync(path.dirname(destination), { recursive: true });
  fs.writeFileSync(destination, value);
};
const mark = (x, y, size, color, outline = false) =>
  `<path d="${symbol.path}" transform="translate(${n(x - 10 * size / 76)} ${n(y - 10 * size / 76)}) scale(${n(size / 76)})" ${outline ? `fill="none" stroke="${color}" stroke-width="${symbol.outlineWidth}" stroke-linejoin="round"` : `fill="${color}"`}/>`;
const text = (x, y, value, size = 15, color = c.muted) =>
  `<text x="${x}" y="${y}" font-family="Helvetica Neue,Arial,sans-serif" font-size="${size}" fill="${color}">${xml(value)}</text>`;
const rect = (x, y, width, height, color) => `<rect x="${x}" y="${y}" width="${width}" height="${height}" fill="${color}"/>`;
const line = (x1, y1, x2, y2) => `<path d="M${x1} ${y1}L${x2} ${y2}" fill="none" stroke="${c.muted}" stroke-width="1"/>`;
const knockout = color => `<path fill="${color}" fill-rule="evenodd" d="M0 0H96V96H0Z ${symbol.path}"/>`;
// macOS app icon template: an 824-unit continuous-corner tile centred on a 1024 canvas with
// the template's drop shadow. macOS 14/15 draw the file as is; later releases mask it again.
const squircle = (centre, half, exponent = 5, steps = 256) => {
  const points = [];
  for (let i = 0; i < steps; i++) {
    const angle = 2 * Math.PI * i / steps;
    const axis = value => Math.sign(value) * Math.abs(value) ** (2 / exponent);
    points.push(`${n(centre + half * axis(Math.cos(angle)))} ${n(centre + half * axis(Math.sin(angle)))}`);
  }
  return `M${points.join('L')}Z`;
};
const appTile = color => '<defs><filter id="shadow" x="-10%" y="-10%" width="120%" height="130%">'
  + '<feDropShadow dx="0" dy="10" stdDeviation="5" flood-color="#000" flood-opacity="0.3"/></filter></defs>'
  + `<path filter="url(#shadow)" fill="${color}" d="${squircle(512, 412)}"/>`;

// SVG is the portable master. Tauri's existing renderer owns raster exports.
for (const [name, color] of Object.entries({ red: c.primary, mono: c.ink, inverse: c.inverse, white: c.white })) {
  write(`identity/symbols/mixrelay-${name}.svg`, svg(`<path fill="${color}" d="${symbol.path}"/>`, 96, 96, `MixRelay ${name} symbol`));
}
for (const [name, color] of Object.entries({ red: c.primary, mono: c.ink, inverse: c.inverse })) {
  write(`identity/symbols/mixrelay-outline-${name}.svg`, svg(`<path d="${symbol.path}" fill="none" stroke="${color}" stroke-width="${symbol.outlineWidth}" stroke-linejoin="round"/>`, 96, 96, `MixRelay ${name} line outline`));
}
for (const [name, color] of Object.entries({ red: c.primary, mono: c.ink, inverse: c.inverse })) {
  write(`identity/symbols/mixrelay-knockout-${name}.svg`, svg(knockout(color), 96, 96, `MixRelay ${name} negative symbol`));
}
// The symbol keeps its share of the tile: 528 of 824, as 656 of the former full-bleed 1024.
write('identity/app-icon.svg', svg(appTile(c.paper) + mark(248, 248, 528, c.primary), 1024, 1024, 'MixRelay app icon'));
write('macos/MixRelay.icon/Assets/symbol.svg', svg(mark(184, 184, 656, c.primary), 1024, 1024, 'MixRelay'));
const appleIcon = JSON.parse(fs.readFileSync(path.join(root, 'macos/MixRelay.icon/icon.json'), 'utf8'));
const solid = color => ({ solid: `srgb:${color.slice(1).match(/../g).map(channel => (parseInt(channel, 16) / 255).toFixed(5)).join(',')},1.00000` });
appleIcon.fill = solid(c.paper);
appleIcon['fill-specializations'] = [{ appearance: 'dark', value: solid(c.dark) }];
appleIcon.groups[0].layers[0]['fill-specializations'] = [{ appearance: 'dark', value: solid(c.inverse) }];
write('macos/MixRelay.icon/icon.json', JSON.stringify(appleIcon, null, 2) + '\n');
write('linux/mixrelay-desktop-symbolic.svg', svg(`<path fill="currentColor" d="${symbol.path}"/>`, 76, 76, 'MixRelay', '10 10 76 76'));
write('tray-white.svg', svg(`<path fill="${c.white}" d="${symbol.path}"/>`, 76, 76, 'MixRelay tray template', '10 10 76 76'));
write('../apps/control-ui/public/favicon.svg', svg(`<path fill="${c.primary}" d="${symbol.path}"/>`, 96, 96, 'MixRelay'));

// Approved symbol-only kit: vector marks and editable document labels.
let board = rect(0, 0, 1200, 1960, c.paper);
board += '<g id="01-primary">' + text(64, 65, 'MIXRELAY / IDENTITY SYSTEM', 18, c.ink) + text(902, 65, 'V1.1 / 2026.10.11', 15);
board += mark(64, 136, 228, c.primary) + mark(420, 198, 96, c.primary);
board += text(64, 414, '01 / OPTICAL GRID', 15, c.ink) + text(420, 340, 'Optical wordmark frozen / separate assets') + '</g>';
board += line(64, 448, 1136, 448);
board += '<g id="02-variants">' + text(64, 494, '02 / FILLED, OUTLINE & NEGATIVE', 15, c.ink);
const xs = [64, 340, 616, 892];
board += mark(xs[0], 535, 112, c.ink) + text(xs[0], 687, 'Monochrome / filled');
board += mark(xs[1], 535, 112, c.primary, true) + text(xs[1], 687, 'Line outline / 2 units');
board += rect(xs[2] - 20, 515, 192, 152, c.dark) + mark(xs[2], 535, 112, c.inverse) + text(xs[2], 687, 'Inverse / light on dark');
board += `<g transform="translate(${xs[3]} 535) scale(${112 / 96})">${knockout(c.primary)}</g>` + text(xs[3], 687, 'Knockout / transparent cut');
board += rect(64, 726, 516, 114, c.dark) + mark(88, 756, 60, c.inverse);
board += mark(646, 756, 60, c.ink) + '</g>';
board += line(64, 880, 1136, 880);
board += '<g id="03-colors">' + text(64, 926, '03 / CORE COLOR ROLES', 15, c.ink);
Object.entries({ 'Brand red / red-500': c.primary, 'Ink / neutral-900': c.ink, 'Paper / neutral-50': c.paper, 'Inverse / graphite-50': c.inverse, 'Dark / graphite-900': c.dark }).forEach(([name, color], i) => {
  const x = 64 + i * 218;
  board += rect(x, 962, 194, 64, color) + `<rect x="${x}" y="962" width="194" height="64" fill="none" stroke="${c.muted}" stroke-width="0.5"/>` + text(x, 1057, name, 14) + text(x, 1083, color.toUpperCase(), 17, c.ink);
});
board += text(64, 1121, 'Source: Core theme/tokens.ts / Brand color identifies the product; status colors keep their own roles.') + '</g>';
board += line(64, 1150, 1136, 1150);
board += '<g id="04-construction">' + text(64, 1178, '04 / CONSTRUCTION & CLEAR SPACE', 15, c.ink);
board += mark(103, 1235, 190, c.ink);
for (let i = 0; i <= 5; i++) board += line(103 + i * 38, 1235, 103 + i * 38, 1425);
board += `<rect x="65" y="1197" width="266" height="266" fill="none" stroke="${c.muted}" stroke-dasharray="4 4"/>`;
board += text(65, 1497, 'H = 5u / u = 15.2 on a 96-unit canvas', 14);
board += text(420, 1253, 'Three stems + two spaces = 5 equal modules', 18, c.ink);
board += text(420, 1290, 'Roof, vertical stems and vertical spaces: 15.2 units');
board += text(420, 1322, 'Diagonal slope: 0.5 / perpendicular width: 13.5953');
board += text(420, 1354, 'Minimum clear space: one module (20% of symbol height)');
board += text(420, 1386, 'Product UI silhouette sizes: 16 / 24 / 64px');
board += text(420, 1418, 'Line outline: same contour, 2-unit stroke, minimum 64px canvas');
board += text(420, 1450, 'Product UI currently uses symbol only') + '</g>';
board += line(64, 1528, 1136, 1528);
board += '<g id="05-product">' + text(64, 1574, '05 / PRODUCT APPLICATION', 15, c.ink);
board += rect(64, 1610, 112, 112, c.paper) + mark(84, 1630, 72, c.primary) + text(64, 1753, 'App / warm paper + red');
board += rect(340, 1610, 112, 112, c.dark) + mark(374, 1644, 44, c.white) + text(340, 1753, 'Tray / white template');
board += mark(616, 1643, 32, c.primary) + text(616, 1753, 'Product header / graphic only');
[16, 24, 32, 48, 64].forEach((size, i) => {
  const x = 70 + i * 120;
  board += mark(x, 1818 - size / 2, size * 76 / 96, c.ink) + text(x, 1872, `${size}px canvas`, 13);
});
board += text(730, 1812, 'Filled master: one closed path', 16, c.ink) + text(730, 1844, 'Small UI: filled, no additional Tab target') + text(730, 1876, 'No stretching, gradients, shadows or doubled strokes') + '</g>';
write('identity/mixrelay-brand-system.svg', svg(board, 1200, 1960, 'MixRelay approved symbol kit v1.1'));
write('identity/manifest.json', JSON.stringify({
  version: '1.1', approvedAt: '2026-10-11', direction: 'B / Optical Grid',
  geometry: 'source/symbol.json', colors: 'apps/control-ui/src/core/theme/tokens.ts#brandColors', colorValues: c,
  approval: 'Optical Grid symbol and frozen Optical wordmark', wordmarkStatus: 'frozen 2026-10-12; wordmark/manifest.json; product screens currently symbol-only', canvas: 96, bounds: [10, 10, 86, 86], module: 15.2,
  clearSpace: '0.2 × silhouette height', minimumCanvas: { filled: 16, outline: 64, knockout: 32 },
  outlineWidth: 2, productSymbolSizes: { compact: 16, standard: 24, display: 64 },
  integratedIntoProduct: true, installedAppVerified: false,
  research: '../logo-study/research/review.json',
}, null, 2) + '\n');
console.log('Brand SVG assets generated from selected geometry and Core colors.');
