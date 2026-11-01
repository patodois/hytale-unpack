# Cloud report: fuzzing `hytale-unpack`

## What was done
- Added `fuzz/` (cargo-fuzz / libFuzzer, nightly, own `[workspace]`, `publish = false`, `exclude = ["/fuzz"]` in the root
  `Cargo.toml`): it is not built by `cargo build/test`, not in the published package, not in CI.
- Seven targets, synthetic inputs only (seeds built by hand from the shapes of the existing tests):

  | target | entry points |
  |---|---|
  | `blockymodel` | `BlockyModel::parse`, `flatten`, `problems`, `pixels_per_unit`, `shape_faces`, `face_uvs` |
  | `blockyanim` | `BlockyAnim::parse`, `problems`, `key_count`, `duration_s` |
  | `prefab` | `prefab::parse`, `stream`, `decode_rotation`, `bridge::block_transform` |
  | `png` | `image::decode_png`, `tint`, `encode_png` |
  | `defs_json` | `defs::merge`, `GradientSets/ModelDef/AttachmentDef/GradientRef/BlockDesc::from_json` |
  | `assets_zip` | `Assets::open` on arbitrary bytes, `read`, `with_reader`, `Defs`, `check::run` |
  | `archive_load` | an archive built from the input (10 slots split on `\n@@\n`), loaded through `Hytale`: entity models, bare models, block items, prefabs, rig, attachments, clips, glTF export |
- Corpus: `fuzz/corpus/<target>/`, minimised with `cargo fuzz cmin`, then restricted to inputs under 1500 bytes
  (2.4 MB in total). Minimised crash inputs are in `fuzz/crashes/<target>/` and also in the corpus.
  Regression tests: unit tests in `src/image.rs`, `src/blockymodel.rs` and `tests/fuzz_regressions.rs`.

## Bugs
1. **`image::decode_png`: allocation from the header.** Input: a 46-byte PNG header declaring 63234 x 4244635652 pixels.
   Failure: `vec![0; output_buffer_size()]` asked for about 1e15 bytes (abort / ASan allocation-size-too-big).
   Fix: refuse when the declared output is over 128 MiB or more than a file of that length can decode to
   (1032 x 32 per input byte: DEFLATE bound from RFC 1951, times the 8-bit RGBA expansion); width x height is computed in `usize`.
   Test: `image::tests::a_header_that_declares_more_than_the_file_can_hold_is_an_error` (+ `a_big_solid_texture_still_decodes`).
   Crash input: `fuzz/crashes/png/huge-dimensions-alloc`. Found by the fuzzer. Note: the `png` crate skips CRC checks when built with
   `cfg(fuzzing)`, so the test builds a PNG with valid CRCs.
2. **`blockymodel::shape_faces`: panic on a multi-byte quad `normal`.** Input: `"normal":"é"` on a quad with a `front` layout.
   Failure: `&normal[len-1..]` is not a char boundary. Fix: look at the last byte. Test:
   `blockymodel::tests::a_quad_normal_that_ends_in_a_wide_character_is_not_an_axis` (checked to fail without the fix).
   Input: `fuzz/crashes/blockymodel/quad-normal-wide-char`. **Found by reading the code, not by the fuzzer**: a 2-minute run
   (2.4 M execs) with a seed one edit away did not hit it.
3. **`Assets::read`: reservation from the central directory.** Input: a ~170-byte zip whose entry claims 250 MiB.
   Failure: `Vec::with_capacity(250 MiB)` (and the read "succeeds" with 20 bytes). Fix: reserve at most
   `min(claimed, compressed_size * 1032)`. Test: `tests/fuzz_regressions.rs::an_entry_that_claims_a_huge_size_does_not_reserve_it`
   (fails without the fix: "reserved 262144000 bytes for a 8 byte entry"). Input: `fuzz/crashes/assets_zip/big-claim`.
   Found while probing by hand; libFuzzer does not flag an untouched large `malloc`, so the target cannot find it by itself.

## Fuzzing time (wall clock, one process each, `-fork=1 -ignore_crashes=1`, `-max_len=16384`, rss limit 2 GB, timeout 10 s)
All seven: 30 minutes in the long run, plus about 1 minute of smoke run (`blockymodel`, `blockyanim`, `prefab`, `png`) and
2 minutes more for `blockymodel`. The long runs found nothing further (0 crashes / OOMs / timeouts). The `png` run
before the fix found bug 1 in the first minute. The long runs of `archive_load`, `assets_zip`, `prefab` and `defs_json`
used binaries built before the `Assets::read` fix (it does not change what they can detect); all of the fixes were in place for the other three.

## Left out / uncertain
- The ask was "at least 30 minutes per target, more while it keeps finding things": every target had 30 minutes and
  found nothing after the fixes, so I did not run longer. `archive_load` runs ~200 execs/s and covers far less than the others;
  a longer run (or a structured generator for the slots) would likely reach more of `rig.rs`/`bridge.rs`.
- Raw-zip mutation rarely gets past the zip crate's checks, so `assets_zip` mostly tests the `zip` crate's own parsing.
- No fuzz target for the CLI (`main.rs`) or for reading real files from disk, and none for `Assets::with_reader` with huge entries (documented design: streaming).
- The 128 MiB PNG cap and the 1500-byte corpus cut are my choices; nothing in the data fixes them (the largest game texture is not known to me: no game data was used).
- No MSRV is declared in the repository; CI uses stable. fmt, clippy (`-D warnings`), `cargo test` and `cargo test --no-default-features` pass locally.
