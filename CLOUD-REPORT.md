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
- Corpus: `fuzz/corpus/<target>/`, minimised with `cargo fuzz cmin <target> -- -set_cover_merge=1 -use_counters=0`
  (the smallest set of inputs that reaches every edge the whole corpus reached): 1796 files, 465 786 bytes
  (it was 10 286 files, 2 378 815 bytes). Minimised crash inputs are in `fuzz/crashes/<target>/` only, not in the corpus.
  Regression tests: unit tests in `src/image.rs`, `src/blockymodel.rs` and `tests/fuzz_regressions.rs`.
- Process notes like this file are kept out of the crate package (`exclude` in `Cargo.toml`).

## Bugs
1. **`image::decode_png`: allocation from the header.** Input: a 46-byte PNG header declaring 63234 x 4244635652 pixels.
   Failure: `vec![0; output_buffer_size()]` asked for about 1e15 bytes (abort / ASan allocation-size-too-big).
   Fix: refuse when the declared output is more than a file of that length can decode to (1032 x 32 per input
   byte: DEFLATE bound from RFC 1951, times the 8-bit RGBA expansion). There is no fixed cap (the first version had one of
   128 MiB; the review showed it refused valid PNGs and nothing in the format justifies it). A non-interlaced image is
   decoded row by row and the output grows with the rows that exist; an interlaced one needs its frame while the passes fill it.
   Tests: `image::tests::a_header_that_declares_more_than_the_file_can_hold_is_an_error`, `the_ratio_of_the_format_refuses_...`,
   `a_one_bit_palette_image_at_the_widest_expansion_decodes`, `an_interlaced_image_is_put_back_in_reading_order`, and in
   `tests/fuzz_regressions.rs` `a_big_valid_png_decodes`, `a_png_cut_short_does_not_reserve_what_its_header_claims`.
   Crash input: `fuzz/crashes/png/huge-dimensions-alloc`. Found by the fuzzer. Note: the `png` crate skips CRC checks when built with
   `cfg(fuzzing)`, so the test builds a PNG with valid CRCs.
2. **`blockymodel::shape_faces`: panic on a multi-byte quad `normal`.** Input: `"normal":"é"` on a quad with a `front` layout.
   Failure: `&normal[len-1..]` is not a char boundary. Fix: look at the last byte. Test:
   `blockymodel::tests::a_quad_normal_that_ends_in_a_wide_character_is_not_an_axis` (checked to fail without the fix).
   Input: `fuzz/crashes/blockymodel/quad-normal-wide-char`. **Found by reading the code, not by the fuzzer**: a 2-minute run
   (2.4 M execs) with a seed one edit away did not hit it.
3. **`Assets::read`: reservation from the central directory.** Input: a ~170-byte zip whose entry claims 250 MiB.
   Failure: `Vec::with_capacity(250 MiB)` (and the read "succeeds" with 20 bytes). Fix: reserve at most
   `min(claimed, min(compressed_size, bytes of the file after the start of the data) * ratio)`, ratio 1 for a stored entry and
   1032 (RFC 1951) for DEFLATE: the compressed size is as untrusted as the uncompressed one, so the file length bounds it.
   Tests: `tests/fuzz_regressions.rs::an_entry_that_claims_a_huge_size_does_not_reserve_it` (DEFLATE entry),
   `a_stored_entry_that_claims_a_huge_size_for_both_sizes_does_not_reserve_it`, `a_stored_entry_reserves_no_more_than_it_stores`.
   Input: `fuzz/crashes/assets_zip/big-claim`.
   Found while probing by hand; libFuzzer does not flag an untouched large `malloc`, so the target cannot find it by itself.

## Fuzzing time (wall clock, one process each, `-fork=1 -ignore_crashes=1`, `-max_len=16384`, rss limit 2 GB, timeout 10 s)
As reported by the original run (no logs are committed, so this is not verifiable from the repository; the independent review
re-ran every target for 11 minutes without a finding): all seven 30 minutes in the long run, plus about 1 minute of smoke run
(`blockymodel`, `blockyanim`, `prefab`, `png`) and 2 minutes more for `blockymodel`. The long runs found nothing further. The `png` run
before the fix found bug 1 in the first minute. The long runs of `archive_load`, `assets_zip`, `prefab` and `defs_json`
used binaries built before the `Assets::read` fix (it does not change what they can detect); all of the fixes were in place for the other three.

## Left out / uncertain
- The ask was "at least 30 minutes per target, more while it keeps finding things": every target had 30 minutes and
  found nothing after the fixes, so I did not run longer. `archive_load` runs ~200 execs/s and covers far less than the others;
  a longer run (or a structured generator for the slots) would likely reach more of `rig.rs`/`bridge.rs`.
- Raw-zip mutation rarely gets past the zip crate's checks, so `assets_zip` mostly tests the `zip` crate's own parsing.
- No fuzz target for the CLI (`main.rs`) or for reading real files from disk, and none for `Assets::with_reader` with huge entries (documented design: streaming).
- `assets_zip` has no structure-aware generator (a valid zip with patched fields): the stored-entry hole of bug 3 was found by
  review, not by fuzzing, and a fuzz oracle for "reserved more than the input justifies" would need a bound the repository does not give.
  The regression tests cover it instead.
- No MSRV is declared in the repository; CI uses stable. fmt, clippy (`-D warnings`), `cargo test` and `cargo test --no-default-features` pass locally.
