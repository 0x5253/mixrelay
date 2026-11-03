import assert from 'node:assert/strict';
import test from 'node:test';
import { keyImage, keyLabel } from './key-image.mjs';

const icon = '<svg width="24" height="24" viewBox="0 0 24 24"><path d="M0 0"/></svg>';
const decode = (image) => decodeURIComponent(image.split(',')[1]);

test('key labels keep a readable prefix and distinguishing Korean suffix within one line', () => {
  const label = keyLabel('MacBook Pro 마이크');
  assert.match(label, /^Mac.*….*이크$/u);
  assert.equal(keyLabel('외장 헤드폰'), '외장 헤드폰');
  assert.equal(keyLabel('A\nB'), 'A B');
  assert.equal(keyLabel('e\u0301'.repeat(20)).includes('\uFFFD'), false);
});

test('volume key has an icon, direction and only the current gain, with XML-safe labels', () => {
  const svg = decode(keyImage({ title: 'A<&"', value: '0.0 dB' }, 'gain_down', icon));
  assert.match(svg, /translate\(52 10\) scale\(1\.666667\)/);
  assert.equal((svg.match(/<svg /g) ?? []).length, 1);
  assert.match(svg, />−<\/text>/);
  assert.match(svg, /A&lt;&amp;&quot;/);
  assert.match(svg, />0\.0 dB<\/text>/);
  assert.equal((svg.match(/<text /g) ?? []).length, 3);
});

test('transport operation is distinguished by shape without a third text row', () => {
  const view = { title: '녹음기', value: '정지' };
  assert.match(decode(keyImage(view, 'pause', icon)), /M62 18v26m20-26v26/);
  assert.match(decode(keyImage(view, 'stop', icon)), /x="59" y="17"/);
  assert.match(decode(keyImage(view, 'record', icon)), /translate\(52 10\)/);
});

test('wide Latin and CJK labels have explicit pixel bounds and invalid XML controls are stripped', () => {
  for (const title of ['WWWWWWWWWWWWWW', 'MMMMMMMMMMMMMM', '가나다라마바사아자차카타파하']) {
    const svg = decode(keyImage({ title, value: 'WWWWWWWWWWWW' }, 'power', icon));
    if ([...keyLabel(title)].length * 16 > 120)
      assert.match(svg, /font-size="16" textLength="120" lengthAdjust="spacingAndGlyphs"/);
    else assert.ok([...keyLabel(title)].length * 16 <= 120);
    assert.match(svg, /font-size="19" textLength="120" lengthAdjust="spacingAndGlyphs"/);
  }
  assert.equal(keyLabel('A\u0000B'), 'AB');
});
