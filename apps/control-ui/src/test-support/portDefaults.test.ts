import { expect, it } from 'vitest';
import { unavailable, unexpected } from './portDefaults';

it('fails unexpected and unavailable port calls with the method name', async () => {
  await expect(unexpected('mutateGraphCatalog')()).rejects.toThrow('unexpected mutateGraphCatalog');
  await expect(unavailable('getNodeControls')()).rejects.toThrow('getNodeControls unavailable');
});
