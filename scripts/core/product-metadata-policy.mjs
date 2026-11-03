export function validateProductMetadata(metadata) {
  if (!metadata || !['publisher', 'email', 'homepage'].every(key => typeof metadata[key] === 'string')
    || !/^[A-Za-z][A-Za-z .-]+$/.test(metadata.publisher)
    || !/^[a-z0-9.+-]+@[a-z0-9.-]+\.[a-z]{2,}$/.test(metadata.email)
    || !/^https:\/\/[a-z0-9.-]+\/[a-z0-9/-]+$/.test(metadata.homepage)) {
    throw new Error('Invalid public product metadata');
  }
}

export const productContact = metadata => `${metadata.publisher} <${metadata.email}>`;
