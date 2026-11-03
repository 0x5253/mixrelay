<h1><picture><source media="(prefers-color-scheme: dark)" srcset="branding/identity/wordmark/mixrelay-lockup-dark.svg"><img src="branding/identity/wordmark/mixrelay-lockup-light.svg" alt="MixRelay" height="56"></picture></h1>

MixRelay is a virtual audio driver and visual router for macOS and Linux.
The editor, background engine and virtual audio devices ship together.
Create multiple virtual audio devices and configure their channels and routing.
On macOS, installation needs no computer restart; system audio briefly stops during installation.

[Download installers](https://github.com/0x5253/mixrelay/releases).

[Website](https://rawsector.com/products/mixrelay) · [User manual](https://rawsector.com/products/mixrelay/manual)

Supports macOS 15+ on Apple Silicon and supported Intel Macs, and Linux
x86_64/arm64 with PipeWire 1.2+ and WirePlumber 0.5+.

## Development

```sh
pnpm install
pnpm desktop:dev
pnpm storybook
```

Run tests for the changed package or behavior. For a focused Rust check:

```sh
node scripts/lab/native-check.mjs cargo test -p PACKAGE --lib TEST_FILTER
```

`pnpm check` runs the integrated checks. `pnpm lab:test` runs target-OS checks.
macOS release builds require full Xcode and configured signing assets.

## Source

- `apps/control-ui`: Core components, tokens, features and screens
- `apps/desktop`: Tauri application and background lifecycle
- `apps/service`: engine composition
- `crates`: platform-neutral contracts, policies and audio processing
- `native`: macOS and Linux adapters
- `packaging`: installers and service definitions

The code defines product behavior. Design values live in Core tokens and Storybook.

## License

[MixRelay Source-Available License](LICENSE.txt) permits personal and business
use and private modifications. Redistribution, resale, rebranding and use in
another product require written permission. [Third-party notices](third-party/README.md)
retain their original license terms.
