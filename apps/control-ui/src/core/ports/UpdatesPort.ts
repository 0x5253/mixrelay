import type { ProductUpdate, UpdateProduct } from '../domain/updates';

/** Desktop checks and installer opening stay behind the native catalog boundary. */
export interface UpdatesPort {
  check(): Promise<ProductUpdate[]>;
  openDownload(product: UpdateProduct, url: string): Promise<void>;
}
