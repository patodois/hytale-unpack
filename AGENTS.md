# Agent guide

hytale-unpack reads Hytale models, animations and prefabs for asset-tool and game-rewrite developers using their own installation.
Its default `bridge` feature supplies a rewrite-bridge provider and glTF export.

## Toolchain, build and CI

- Stable Rust, edition 2021; no minimum Rust version is declared. The README reports Rust 1.99.
- CI checks Linux, macOS and Windows, with and without the default `bridge` feature.
- Exact CI commands from the repository root:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo test --no-default-features
sh tools/check-package.sh
```

The README builds the executable with `cargo build --release`.
The package check compares `cargo package --list` with `tools/package-files.txt`; update the list with package changes.
Release CI builds with `cargo build --release --locked --target <target> -p hytale-unpack --features bridge`.
Its targets are `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`.

## Code map

- `src/assets.rs`: zip index and bounded entry reads.
- `src/blockymodel.rs`, `blockyanim.rs`, `prefab.rs`: models, clips and building cells.
- `src/defs.rs`, `rig.rs`, `image.rs`: definitions, hierarchy and textures.
- `src/bridge.rs`, `gltf.rs`: provider conversion and export.
- `src/main.rs`, `check.rs`, `tests/`: CLI, checks and synthetic/game-data regressions.
- `render/`, `fuzz/`: optional rendering scripts and separate fuzz targets.

## Project rules

- Follow the README's rule: nothing invented, 1:1 with game data.
- Leave out and document behavior that neither data nor the official Blockbench plugin establishes.
- Preserve the README's attribution to the official Blockbench plugin as a format source.
- Read the archive's central directory and individual entries; never load or extract the whole archive.
- `assets.rs` limits an entry's actual decoded bytes to 256 MiB, independent of claimed zip sizes.
- Game tests use `HYTALE_ASSETS` or the known macOS install and skip without either.
- Keep models, textures, icons and rendered outputs outside the repository.

## Public files

- Never commit game files, extracted assets, renders, decompiled code or secrets.
- The public-tree gate needs Python 3.9+ and Git; run it from the repository root:
  `python3 tools/check_public_tree.py` (checks Git's index, including staged new files).
- CI checks committed changes with `python3 tools/check_public_tree.py --range BASE..HEAD`.

## Contributing, license and AI

- Open an issue to discuss a problem or send a pull request with the change.
- Before a pull request, run the applicable CI commands above and the public-tree gate.
- License: MIT OR Apache-2.0, at your option; see `LICENSE-MIT` and `LICENSE-APACHE`.

The code was written with AI coding agents (Anthropic's Claude and OpenAI's GPT models) under the maintainer's direction.
AI-assisted contributions are welcome when their use is disclosed and the changes are reviewed.
