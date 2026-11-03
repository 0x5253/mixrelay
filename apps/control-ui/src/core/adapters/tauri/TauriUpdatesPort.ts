import type { ProductUpdate, UpdateDownload, UpdateProduct } from '../../domain/updates';
import type { UpdatesPort } from '../../ports/UpdatesPort';
import { array, literal, nonNegativeInteger, nullable, object, string } from '../decode';
import { type NativeInvoke, tauriInvoke } from './nativeInvoke';

const productUpdates = array(
  object<ProductUpdate>({
    product: literal('mix-relay'),
    installedVersion: nullable(string),
    latestVersion: nullable(string),
    state: literal('available', 'current', 'published', 'not_published', 'unavailable'),
    downloads: array(
      object<UpdateDownload>({
        os: string,
        arch: string,
        url: string,
        sha256: string,
        size: nonNegativeInteger,
      }),
    ),
    notesUrl: nullable(string),
  }),
);

export class TauriUpdatesPort implements UpdatesPort {
  constructor(private readonly invoke: NativeInvoke = tauriInvoke) {}

  async check(): Promise<ProductUpdate[]> {
    return productUpdates(await this.invoke('updates_check'), 'updates_check');
  }

  async openDownload(product: UpdateProduct, url: string): Promise<void> {
    await this.invoke('updates_open_download', { product, url });
  }
}
