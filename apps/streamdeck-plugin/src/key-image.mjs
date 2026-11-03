const segments = new Intl.Segmenter(undefined, { granularity: 'grapheme' });
const width = (text) => ([...text].some((character) => character.codePointAt(0) > 255) ? 2 : 1);

/** Keep both the device prefix and distinguishing suffix inside a key's text area. */
export function keyLabel(value, budget = 14) {
  const clean = value
    .replace(/\s+/gu, ' ')
    .replace(/\p{Cc}/gu, '')
    .trim();
  const parts = [...segments.segment(clean)].map((part) => part.segment);
  if (parts.reduce((total, part) => total + width(part), 0) <= budget) return parts.join('');
  let left = '';
  let right = '';
  let remaining = budget - 2;
  while (parts.length && remaining >= 2) {
    const first = parts.shift();
    left += first;
    remaining -= width(first);
    if (parts.length && remaining >= 2) {
      const last = parts.pop();
      right = last + right;
      remaining -= width(last);
    }
  }
  return `${left}…${right}`;
}

function boundedText(value, y, fontSize, budget) {
  const text = keyLabel(value, budget);
  // An em per code point is conservative for the chosen font, including wide Latin.
  const bounds =
    [...text].length * fontSize > 120 ? ' textLength="120" lengthAdjust="spacingAndGlyphs"' : '';
  return `<text x="72" y="${y}" font-size="${fontSize}"${bounds}>${xml(text)}</text>`;
}

function xml(text) {
  return text.replace(
    /[<>&"']/gu,
    (character) =>
      ({
        '<': '&lt;',
        '>': '&gt;',
        '&': '&amp;',
        '"': '&quot;',
        "'": '&apos;',
      })[character],
  );
}

/** Own text bounds and icon placement instead of inheriting per-key title styling. */
export function keyImage(view, operation, icon) {
  const glyph = icon
    .replace(
      /^<svg[^>]*>/u,
      '<g transform="translate(52 10) scale(1.666667)" fill="none" stroke="#ccd5d6" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">',
    )
    .replace('</svg>', '</g>');
  const mark = operation === 'gain_down' ? '−' : operation === 'gain_up' ? '+' : '';
  const transport =
    operation === 'pause'
      ? '<path d="M62 18v26m20-26v26" stroke-width="7"/>'
      : operation === 'stop'
        ? '<rect x="59" y="17" width="26" height="26" fill="currentColor" stroke="none"/>'
        : glyph;
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="144" height="144" viewBox="0 0 144 144">
<rect width="144" height="144" fill="#191e20"/>
<g color="#ccd5d6" stroke="currentColor">${transport}</g>
<g fill="#edf1f1" font-family="Arial, sans-serif" text-anchor="middle">
<text x="112" y="41" font-size="27">${mark}</text>
${boundedText(view.title, 82, 16, 14)}
${boundedText(view.value, 117, 19, 12)}
</g></svg>`;
  return `data:image/svg+xml,${encodeURIComponent(svg)}`;
}
