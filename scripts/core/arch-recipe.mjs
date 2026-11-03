export function renderArchRecipe({ version, architecture, checksum }, { template, metadata, minimums }) {
  const arch = { amd64: 'x86_64', arm64: 'aarch64' }[architecture];
  if (!arch || !/^\d+\.\d+\.\d+$/.test(version) || !/^[a-f0-9]{64}$/.test(checksum)) {
    throw new Error('Invalid Arch release version, architecture or payload checksum');
  }
  return template
    .replaceAll('@HOMEPAGE@', metadata.homepage)
    .replaceAll('@CONTACT@', metadata.contact)
    .replaceAll('@VERSION@', version).replaceAll('@ARCH@', arch).replaceAll('@SHA256@', checksum)
    .replaceAll('@GLIBC@', minimums.linux.glibc)
    .replaceAll('@PIPEWIRE@', minimums.linux.pipewire)
    .replaceAll('@WIREPLUMBER@', minimums.linux.wireplumber);
}
