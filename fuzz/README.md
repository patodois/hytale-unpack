# Fuzzing

Coverage-guided fuzz targets (cargo-fuzz / libFuzzer) for every reader that takes untrusted bytes.
Nightly Rust is needed here only; this crate is not part of the root workspace, the published package or CI.

```sh
cargo install cargo-fuzz
cargo +nightly fuzz run blockymodel            # from the repository root
cargo +nightly fuzz run archive_load -- -rss_limit_mb=2048 -timeout=10
```

| target | entry points |
|--------|--------------|
| `blockymodel` | `BlockyModel::parse`, `flatten`, `problems`, `pixels_per_unit`, `shape_faces` |
| `blockyanim` | `BlockyAnim::parse`, `problems`, `key_count`, `duration_s` |
| `prefab` | `prefab::parse`, `prefab::stream`, `decode_rotation`, `bridge::block_transform` |
| `png` | `image::decode_png`, `tint`, `encode_png` |
| `defs_json` | `merge`, `GradientSets::from_json`, `ModelDef::from_json`, `AttachmentDef::from_json`, `BlockDesc::from_json` |
| `assets_zip` | `Assets::open` on arbitrary bytes, `read`, `with_reader`, `Defs`, `check::run` |
| `archive_load` | a small archive built from the input, loaded through `Hytale` (entity models, bare models, prefabs, rig, clips, glTF) |

Inputs are synthetic only. `corpus/<target>/` holds a minimised corpus (`cargo fuzz cmin`), and
`crashes/` the minimised inputs of bugs that were found and fixed (each has a regular test).
