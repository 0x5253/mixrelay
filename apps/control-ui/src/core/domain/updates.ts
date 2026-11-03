export type UpdateProduct = 'mix-relay';

export interface UpdateDownload {
  os: string;
  arch: string;
  url: string;
  sha256: string;
  size: number;
}

export interface ProductUpdate {
  product: UpdateProduct;
  installedVersion: string | null;
  latestVersion: string | null;
  state: 'available' | 'current' | 'published' | 'not_published' | 'unavailable';
  downloads: UpdateDownload[];
  notesUrl: string | null;
}
