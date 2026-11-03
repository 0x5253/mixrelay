<h1><picture><source media="(prefers-color-scheme: dark)" srcset="branding/identity/wordmark/mixrelay-lockup-dark.svg"><img src="branding/identity/wordmark/mixrelay-lockup-light.svg" alt="MixRelay" height="56"></picture></h1>

MixRelay is a free virtual audio driver and visual router for macOS and Linux. Create virtual
audio devices, connect apps, microphones and effects in a graph, and record the result.
The editor, background engine and virtual devices ship in one installer.

[Website](https://rawsector.com/products/mixrelay) · [User manual](https://rawsector.com/products/mixrelay/manual) · [Buy Me a Coffee](https://buymeacoffee.com/rawsector)

## Install

Download from [Releases](https://github.com/0x5253/mixrelay/releases/latest) and check files against `SHA256SUMS`.

| System | File |
| --- | --- |
| macOS 15+ (Apple Silicon; Intel up to macOS 26) | `MixRelay-Installer-<version>.pkg` (Universal, notarized) |
| Debian 13, Ubuntu 26.04, Kali (x86_64, arm64) | `MixRelay_<version>_amd64.deb`, `MixRelay_<version>_arm64.deb` |
| Arch Linux (x86_64) | `mix-relay-<version>-1-x86_64.pkg.tar.zst` |

Linux needs PipeWire 1.2+ and WirePlumber 0.5+ in a normal user session.

## Development

```sh
pnpm install
pnpm desktop:dev   # editor with hot reload
pnpm storybook     # Core components and tokens
pnpm check         # integrated checks
```

macOS builds need macOS 26 and Xcode 26. Linux builds run in `infra/lab` containers.

## Contributing

Open a pull request; it merges by squash once its required checks pass. Use a
[Conventional Commits](https://www.conventionalcommits.org) title: it becomes the commit
message and the changelog entry. `chore`, `ci` and `docs` pull requests skip builds.
Pull requests build every installer unsigned. Merged changes collect in a release PR;
merging it publishes the signed release.

## License

[MixRelay Source-Available License](LICENSE.txt): free for personal and business use, with
private changes allowed. Redistribution, resale, rebranding and use in another product need
written permission. Third-party components keep their own licenses:
[notices](third-party/THIRD-PARTY-NOTICES.txt).
