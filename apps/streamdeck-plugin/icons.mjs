import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { Resvg } from '@resvg/resvg-js';

export async function generateIcons(output, marketplace) {
  const brand = new URL('../../branding/identity/', import.meta.url);
  const app = await readFile(new URL('app-icon.svg', brand), 'utf8');
  for (const [suffix, size] of [['', 256], ['@2x', 512]]) {
    const png = new Resvg(app, { fitTo: { mode: 'width', value: size } }).render().asPng();
    await writeFile(`${output}/images/app${suffix}.png`, png);
  }
  if (marketplace) {
    await mkdir(marketplace, { recursive: true });
    await writeFile(`${marketplace}/app-icon.png`,
      new Resvg(app, { fitTo: { mode: 'width', value: 288 } }).render().asPng());
  }
  const symbol = (await readFile(new URL('symbols/mixrelay-white.svg', brand), 'utf8'))
    .replace('width="96" height="96"', 'width="28" height="28"')
    .replace('#ffffff', '#FFFFFF');
  await writeFile(`${output}/images/icon.svg`, symbol);
}
