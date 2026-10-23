# hytale-unpack

A small Rust reader for the game data of Hytale, read from your own install:

- `Assets.zip` (3.5 GB, 60,695 entries), through its central directory and single entries, never whole;
- `.blockymodel` (2,830 models: node trees of boxes and quads with texture layouts);
- `.blockyanim` (6,736 clips);
- prefabs (`Server/Prefabs/**.prefab.json`, 7,828 buildings, 20.6 million block cells);
- the JSON definitions that link them: entity models (`Server/Models`), items and blocks (`Server/Item/Items`);
- a [rewrite-bridge](https://github.com/patodois/rewrite-bridge) `Provider` for all of it, so other game
  rewrites can load Hytale models, animations and buildings; and a glTF 2.0 exporter.

This project contains no game code or game data, and it is not affiliated with, endorsed by or
sponsored by Hypixel Studios. Hytale is a trademark of its owners. It only reads files from a copy of
the game you already have installed. See [Legal](#legal).

The rule of this crate is **nothing invented, 1:1 with the game data**: whatever cannot be determined
from the data or from the official Blockbench plugin is left out, and listed under
[What is left out](#what-is-left-out).

## What you need

- **Hytale, installed with the official launcher.** This tool reads `Assets.zip` from your own copy; it ships
  no game data. Default location of that file, which `hytale-unpack` looks at when you give no path:

  | OS | `Assets.zip` |
  |----|--------------|
  | macOS | `~/Library/Application Support/Hytale/install/release/package/game/latest/Assets.zip` |
  | Windows, Linux | no built-in default: find `Assets.zip` in the launcher's install folder (the same `install/release/package/game/latest/` layout, under the launcher's data folder) and point to it |

  On any system, `export HYTALE_ASSETS=/path/to/Assets.zip` (or pass `--assets /path/to/Assets.zip` before the
  subcommand) overrides the default. The Windows and Linux paths have not been tried by the author.
- **Rust**, the stable toolchain from [rustup.rs](https://rustup.rs) (edition 2021; written and tested with
  1.99), and `git`, because Cargo fetches [rewrite-bridge](https://github.com/patodois/rewrite-bridge) from GitHub.
  A first release build takes about 10 seconds on an M-series Mac with 4 jobs (`cargo build --release -j 4`),
  plus the dependency download.
- **Disk and memory.** Nothing is extracted: the 3.5 GB archive is read entry by entry. The commands below
  need between 100 MB and 350 MB of RAM (numbers beside each command), so a small laptop is fine.

## Words used here

- **`Assets.zip`**: the archive that holds the game's models, textures, sounds and JSON data. An *id* in this tool
  is an entry path inside it.
- **`.blockymodel`, `.blockyanim`**: Hytale's model (a tree of boxes and flat quads) and animation (keyframes per
  node) files. A *clip* is one animation. A *prefab* is a stored building: a list of block cells.
- **Entity model, block item**: JSON definitions that link a model to its texture and clips (`Server/Models`), or a
  model to a block (`Server/Item/Items`).
- **Attachment**: a separate model worn by another one (eyes, hair, hats). See [What is left out](#what-is-left-out).
- **glTF**: the open 3D file format the exporter writes; any viewer or Blender opens it.
- **[rewrite-bridge](https://github.com/patodois/rewrite-bridge)**: a small library with common model types, so
  rewrites of different games can load each other's assets into one scene. A **`Provider`** is a game's reader
  that implements it; this crate has one for Hytale. To write one for another game, see
  [docs/WRITING_A_PROVIDER.md](https://github.com/patodois/rewrite-bridge/blob/main/docs/WRITING_A_PROVIDER.md).
- **Other providers**: [nocturne-unpack](https://github.com/patodois/nocturne-unpack) (Shin Megami Tensei III
  Nocturne HD Remaster), [wf-unpack](https://github.com/patodois/wf-unpack) (Warframe) and
  [wt-unpack](https://github.com/patodois/wt-unpack) (War Thunder) read their games the same way and return the same
  types, so a Hytale model can stand next to their characters.
- **[bridge-testbed](https://github.com/patodois/bridge-testbed)**: a [Bevy](https://bevy.org) app that loads the
  assets of every provider side by side, to check scale, orientation and animation across games by eye. It needs your
  own game installs; no game data is in it.
- **Conformance checker**: the bridge's test that a provider's output has the right units, orientation and
  winding; see [Checking the provider](#checking-the-provider).
- **RSS**: the memory a process holds; "peak RSS" is the most it held.

## Build and use

```sh
cargo build --release          # the `bridge` feature (Provider, glTF, model/prefab export) is on by default
cargo build --release --no-default-features   # parsers, `anim`, `prefab` (print only), `check`

export PATH=$PWD/target/release:$PATH   # so `hytale-unpack` works from anywhere (this shell only);
                                        # or write `cargo run --release --` in front of each command below
# Assets.zip: $HYTALE_ASSETS, else the macOS launcher's install (see above); --assets <file> overrides

hytale-unpack list Kweebec                # ids of models, entity models, block items and prefabs containing the text
hytale-unpack list Kweebec --raw          # raw archive entries instead (this is where clips show up)
hytale-unpack model Human/Player -o out/  # entity model + its clips -> out/Player.gltf, Player.bin, Player_0.png
hytale-unpack model Server/Models/Intelligent/Trork/Trork.json --clips Walk -o out/
hytale-unpack model Bench_WorkBench -o out/                  # a block item (a block with a texture per side, such as Soil_Grass, exports too)
hytale-unpack model Common/NPC/Beast/Bear_Grizzly/Models/Model.blockymodel   # a bare model, print only
hytale-unpack list Walk --raw | grep Animations/Default       # find a clip id ...
hytale-unpack anim Default/Walk           # ... and print it: frames, nodes, keyframes (--keys lists them)
hytale-unpack prefab Server/Prefabs/Npc/Kweebec/Autumn/Houses_Small/Kweebec_Autumn_Houses_Small_001.prefab.json
hytale-unpack prefab <path> -o out/       # the same, plus the building as glTF in out/<prefab name>/ (one folder per prefab)

hytale-unpack check                       # parse every model, clip and prefab (streaming); counts and peak RSS
hytale-unpack check-bridge --scenes 30    # load everything through the Provider and validate it (prefabs under 30 MB)
hytale-unpack check-conformance --filter Server/Models/Beast/ --limit 20   # check what the Provider returns against the bridge's conventions
```

**Clip ids.** A clip is addressed by the folder under `Common/Characters/Animations/` plus its name, such as
`Default/Walk` (the player's walk); a bare `Walk` is not enough, because the archive has dozens of them
(`Items/Dual_Handed/Battleaxe/Walk`, `Default/Crouch_Walk`, ...). `list <text> --raw` shows the paths, and
`anim` with an id it cannot find prints the paths that contain what you typed. `list` without `--raw` shows
models, blocks and buildings, never clips; the clips of one model are in the output of `model <id>` (the
`--clips <text>` filter works there too).

**Time and memory** (release build, M-series Mac, 16 GB, 23 October 2026, with other builds running; "peak" is the
peak resident size): `list` 0.2 s and 120 MB (1 s when the archive's index of 60,695 entries is not in the file cache);
`model` under 0.5 s and 90 MB; `check` about 5 s and 80 MB; `check-bridge --scenes 30` about 25 s and 300 to 390 MB (the
23 MB prefab dominates); `check-conformance` over everything about 25 s and 400 MB. Nothing loads the archive whole.

The output of `list` goes through a pipe quietly (`hytale-unpack list Kweebec | head`).

Ids are entry paths in `Assets.zip`; the commands also accept the file stem of an item or entity model
(`Rock_Stone`, `Player`) and paths without the leading folder.

Tests (`cargo test --release`) run against a real install when `HYTALE_ASSETS` points at it, or the default
install exists, and skip otherwise.

## The install this was written against

`~/Library/Application Support/Hytale/install/release/package/game/latest/`, 3.8 GB: `Assets.zip`,
`Client/`, `Server/` (including `HytaleServer.jar`, which this crate does not touch).

Every `Assets.zip` entry is *stored* (method 0), so reading an entry is a seek and a copy. Entries are flat
paths: `Common/` (what the client needs: `Blocks`, `Characters`, `Cosmetics`, `Items`, `NPC`, `Icons`, `Sounds`,
textures), `Server/` (JSON game data: `Models`, `Item`, `NPC`, `Prefabs`, `World`, `Particles`, `Audio`, ...) and
`Cosmetics/`. By extension: 32,936 `.json`, 10,901 `.png`, 4,490 `.ogg`, 2,830 `.blockymodel`, 6,736
`.blockyanim`, 1,762 `.particlespawner`, 602 `.particlesystem`. `Server/Prefabs` holds 7,829 JSON files:
7,828 prefabs and one `BranchInfo.json`.

## Formats as decoded

The module docs are the reference: [`src/assets.rs`](src/assets.rs), [`src/blockymodel.rs`](src/blockymodel.rs),
[`src/blockyanim.rs`](src/blockyanim.rs), [`src/prefab.rs`](src/prefab.rs), [`src/defs.rs`](src/defs.rs),
[`src/rig.rs`](src/rig.rs). In short:

**`.blockymodel`** is JSON: `nodes` is a tree of `{id, name, position, orientation, shape, children}`.
Lengths are pixels, 64 per unit for characters and attachments, 32 for props and blocks (chosen by the file's
`format` field, else by a `Blocks` path segment, as the Blockbench plugin does). A node's `position` is its pivot
relative to its parent's pivot **plus the parent's `shape.offset`**; it rotates about the pivot. A `shape` is a
`box` (`size` x,y,z) or a `quad` (`size` x,y and a `normal` such as `+Z`) or `none`, drawn centred on
`pivot + offset`, scaled by `stretch` about that centre (negative mirrors, children are not affected).
`textureLayout` gives each face (`front` +Z, `back`, `left` -X, `right` +X, `top`, `bottom`) a pixel offset
into the texture, a `mirror` and an `angle` in quarter turns; faces without an entry are not drawn; a quad
reads `front`. UVs are pixels of the texture, divided by that texture's own size.

**`.blockyanim`** is JSON: `duration` in frames at **60 fps**, `holdLastKeyframe`, and `nodeAnimations`
keyed by node name with `position`, `orientation`, `shapeStretch`, `shapeVisible` and `shapeUvOffset` keyframes
`{time (frames), delta, interpolationType: "smooth" | "linear"}`. Values are deltas on the rest pose:
position adds, orientation composes after the rest orientation, stretch multiplies.

**Prefabs** are `{version: 8, blockIdVersion: 11, anchorX/Y/Z, blocks: [{x, y, z, name, rotation?, filler?,
support?, components?}], entities, fluids}`. `name` is an item id. A cell named `*Rail_State_Definitions_Corner_Left`
is the item `Rail` in state `Corner_Left`; `Empty` is air. They are streamed: the biggest is 23 MB of JSON.

**Definitions** use `"Parent": "<id>"` inheritance (objects merge recursively, everything else is replaced); a
`BlockType` can have its own `Parent` too. `Server/Models/**.json` give an entity's `Model`, `Texture` and
`AnimationSets`; items with a `BlockType` are blocks (`DrawType` `Cube`, `Model`, `CubeWithModel`).

The Blockbench plugin's source ([JannisX11/hytale-blockbench-plugin], GPL-3.0, copyright Hypixel Studios
Canada Inc.) was read to learn what the fields mean (units, UV mapping, hierarchy, animation semantics).
This crate contains none of its code; the formulas are re-implemented from the behaviour, with unit tests.
Credit for the format knowledge goes to that plugin and its authors.

## Provider

`hytale_unpack::bridge::Hytale` implements `rewrite_bridge::Provider` (rewrite-bridge `v0.1.0`). Conventions are the
bridge's: metres, +Y up, right-handed, +Z front, counter-clockwise front faces, sRGB textures. One block (the
game's unit) is taken as one metre, which the data does not state; it fits the player (hitbox 1.85 high,
eye height 1.6, model about 1.89 tall).

To use it from another crate, depend on both, and keep **rewrite-bridge on the same tag this crate uses**
(`v0.1.0`): with two different versions of rewrite-bridge in one build, `Hytale` implements a `Provider` that
your code's `Provider` is not, and the compiler reports "multiple different versions of crate `rewrite_bridge`".
`cargo tree -i rewrite-bridge` must show a single copy.

```toml
[dependencies]
hytale-unpack = { git = "https://github.com/patodois/hytale-unpack" }   # add rev = "<commit>" to pin it
rewrite-bridge = { git = "https://github.com/patodois/rewrite-bridge", tag = "v0.1.0" }
```

```rust
let hy = hytale_unpack::bridge::Hytale::open_default()?;
let ids = hy.list("Trork")?;                                   // ids are Assets.zip paths
let trork = hy.load_animated("Server/Models/Intelligent/Trork/Trork.json")?;
let clip = trork.animation("Walk").unwrap();
let model = hy.load("Server/Item/Items/Bench/Bench_WorkBench.json")?;
let house = hy.load_scene("Server/Prefabs/Npc/Kweebec/Autumn/Houses_Small/Kweebec_Autumn_Houses_Small_001.prefab.json")?;
```

| id | kind | `load` / `load_animated` / `load_scene` |
|----|------|-----------------------------------------|
| `Server/Models/**.json` with a `Model` | Character | model + texture; clips from `AnimationSets` |
| `Common/**.blockymodel` | Character, Weapon, Prop, Other by folder | model + the texture found next to it; clips by folder rule |
| `Server/Item/Items/**.json` with a drawable `BlockType` | Prop | the block: textured cube or custom model, origin at the bottom centre of its cell |
| `Server/Prefabs/**.prefab.json` | Level | `load_scene` only: one instance per block cell |

- **Hierarchy.** One bridge node per Hytale node (its pivot), named after it; a node with a visible shape has one
  child `<name>/shape` carrying the shape's offset and stretch, and the mesh (one per shape, one triangle list of
  quads). Shapes with `visible: false` have no mesh. Materials: the texture, matte (`metallic 0`, `roughness 1`),
  in variants for double-sided shapes and for shapes with a `transparent` face; a texture with transparent texels is
  alpha-masked at 0.5, a `transparent` face blends.
- **Which clips belong to a model** (`load_animated`):
  1. an entity model's `AnimationSets` (with `Parent` inheritance, so the Trork gets the player's sets unless it
     overrides them): a one-clip set is named after the set (`Walk`), a bigger one `Set/<file>`. A set may list
     one file several times, told apart only by `Speed` (the bat's `Fly`: default, 1.25, 1.5); those clips are
     `Fly/Fly`, `Fly/Fly@1.25` and `Fly/Fly@1.5`, and entries equal in file and speed get `#2`, `#3`;
  2. for a bare model `<dir>/<name>.blockymodel`, the plugin's rule: every `.blockyanim` in each folder of
     `<dir>/../Animations/`, named `<folder>/<file>`.
  Clips address nodes by name and drive every node of that name; nodes the model lacks are skipped. A clip the
  bridge would reject (a key at a negative time, say) is left out and named in `LoadReport::skipped_clips`, and
  `model` prints it; the model still loads.
- **Clips become channels:** `position` -> translation (rest + delta, metres), `orientation` -> rotation (rest *
  delta, spherical), `shapeStretch` -> scale of the `<name>/shape` node (stretch * delta). `smooth` position and
  stretch keyframes become cubic-spline keyframes reproducing the plugin's Catmull-Rom preview; rotations are always
  spherical (the plugin does this for Hytale formats). Times are frames / 60.
- **Scenes** hold one inline model per distinct block and one instance per cell: cell `(x, y, z)` is
  `[x, x+1] x [y, y+1] x [z, z+1]`, moved by minus the anchor, so the anchor cell is `[0, 1]` in x, y and z (its
  floor at y = 0, the origin at its lowest corner) and an unrotated block, whose model origin is its bottom centre,
  is at `(x - ax + 0.5, y - ay, z - az + 0.5)`. The data decides this: a prefab's entities are in the same frame as its cells. Of the 743 entities that are not
  spawn markers and stand at an integer y (in the prefabs under 3 MB), 680 have a block under their feet by either
  rule, 26 only if the cell is found by rounding down, none only by rounding to the nearest, and 37 neither (a mannequin
  at x = 2.85 stands on cell 2, which a cell centred on its coordinate would make cell 3). `rotation` is `yaw + 4 * pitch (+ 16 * roll)` in quarter turns, applied about the cell centre
  as `Ry(yaw) * Rx(pitch) * Rz(roll)`. Cells with a `filler` (parts of bigger blocks) and `Empty` cells are not drawn.
  `load_scene_report` also returns what was skipped.
- **What is listed.** Only what draws something. Definitions in `Server/Models` without a `Model` (the showcase
  mannequins that are only particles) and items whose `BlockType` draws nothing are not listed: `DrawType: Empty`
  (`Barrier`), and the editor's brush filters and prefab placers (`Filter_TreeWoodAndLeaves`, `Filter_Rock`,
  `ExamplePrefabPlacementBlock_Tree`), whose `BlockType` only names a block list or a prefab list
  (`BlockListAssetId`, `PrefabListAssetId`) for the tool to use. Prefab cells can still name any block.

## Checking the provider

`check-conformance` runs the bridge's conformance checker over what the provider returns, one asset at a time:
scale, orientation, winding, skins, materials, clip names and durations, scenes. Options are the checker's
(`--id` repeatable, `--filter`, `--kind`, `--limit`, `--compare-load`, `--allow`, `-v`, `--strict`; `--help`
lists them); put `--assets` before the subcommand. Exit status 1 means an asset has an error.

```sh
hytale-unpack check-conformance --filter Server/Models/Beast/ --limit 20   # 20 creatures (entity models)
hytale-unpack check-conformance --id Server/Models/Beast/Rat.json -v       # one creature, with the info findings
```

(`--kind character` would list the attachment models first: beards, hair and ears, which are bare pieces.)

On the install this was written against, every listed model, block and prefab loads, and no clip name repeats:
0 errors. A full run (`hytale-unpack check-conformance`, 14,037 assets, about 25 s, peak RSS 400 MB) ends with this
table of warnings, which come from the game's data or from what is left out, not from the loading:

| warning | count | assets | what it means here |
|---|---:|---:|---|
| `animation.zero_scale` | 1,431 | 193 | clips hide a node by scaling it to zero: the blink (`L-Eye`, eyelids) in nearly all of them |
| `orientation.feet_off_ground` | 541 | 541 | the lowest point is more than 0.3 m from the ground: attachment pieces modelled at head height (hair, beards, hats, eyes, ears) and a few flying creatures (bat, hawk, raven) |
| `animation.empty` | 459 | 459 | clips with no channels because they only move nodes the model lacks: eyes and hair (separate attachment models, not composed), or another block's animation such as `Candle_Burn`, which a block model picks up from its folder |
| `geometry.degenerate_triangles` | 129 | 20 | triangles with no area: boxes of zero thickness in hair and costume pieces and a few bones |
| `scale.height_out_of_range` | 103 | 103 | under 0.2 m high: small pieces (eyebrows, shoes, gloves, a rattle) |
| `scene.empty` | 50 | 50 | prefabs whose cells are all `Empty` (they carve holes in terrain, e.g. `Klops_Basalt_Cave_001`) |
| `front.not_plus_z` | 50 | 48 | node names such as `Front`, `Nose` or `Eye` that point along +X or -X (24 of the assets are prefabs, i.e. block models inside them); the data does not say which way a block faces, so they are left as they are |
| `orientation.not_upright` | 25 | 25 | flat pieces (ears, earrings, eyebrows, bracers) that are wider than tall |
| `scale.too_small` | 2 | 2 | the two earring models |
| `geometry.no_vertices` | 1 | 1 | `Common/NPC/MISC/Empty.blockymodel`, an empty model on purpose |

Counts are findings, assets are the assets with at least one (an asset can have several of one code).

## Rendering checks

`render/` has a three.js + Playwright script that renders an exported glTF headless in Chromium. It needs
[Node.js](https://nodejs.org) 20 or newer (tried with 24.16; `node -v` tells yours), and a browser for Playwright
to drive, which `playwright-core` does not download by itself (without it the script stops with "Executable doesn't
exist"):

```sh
cd render
npm install                          # playwright-core and three, into render/node_modules
npx playwright-core install chromium # the browser, about 580 MB in ~/Library/Caches/ms-playwright (once)
node shot.cjs out.png model.gltf "anim=Walk&frac=0.33&fit=1.0&dir=0.6,0.3,1" 800x700
```

(`CHROMIUM=/path/to/chrome` uses a browser you already have instead of the download.) The model is the `.gltf` that
`hytale-unpack model <id> -o <dir>` wrote, and the script prints what it drew (`{"tris":12,"nanim":0,...}`).
Used to check by eye (exported glTF files and renders are game content and are not in this repository):

- character: Trork (`Server/Models/Intelligent/Trork/Trork.json`) and the player;
- animation: Trork `Walk` and player `Run`, three frames each;
- block: `Bench_WorkBench`;
- building: `Encounter_Forest_Townhall_001` (4,092 cells, 43 block names) and a Kweebec tree house.

## Numbers

`hytale-unpack check` on the install above (release build, M-series Mac):

| | ok | failed |
|---|---:|---:|
| `.blockymodel` | 2,830 | 0 |
| `.blockyanim` | 6,736 | 0 |
| prefabs | 7,828 | 0 |

39,440 nodes, 452,392 keyframes, 20,559,261 block cells (37,367 fillers), 207,019 fluid cells, 4,441 entities.
Prefab versions: all `version 8` / `blockIdVersion 11`. Block `rotation` values seen: 1 to 11 only. Two clips have
a keyframe past their `duration` (noted, not an error). **Peak RSS 79 MB, about 5 s** (`/usr/bin/time -l`: 79 MB).

`check-bridge --scenes 30` loads all 436 entity models with a model (69,323 clips built), all 2,830 bare models,
all 2,956 block items and all 7,828 prefabs as scenes (14.75 million instances), validating each: 0 failures,
no unresolved block names. Peak RSS 300 to 390 MB (the 23 MB prefab dominates), about 25 s.

## What is left out

- **Attachments** (`isPiece`, `DefaultAttachments`: eyes, hair, cloth, hats). The player's face and a Kweebec's
  eyes are separate attachment models; they are reported by `model` and not composed. Clips that animate their
  nodes (`Hair-B`, `L-Eye`, ...) are reported as missing nodes.
- `shapeVisible` and `shapeUvOffset` keyframes (the bridge has no visibility or UV targets), `holdLastKeyframe`
  and looping, clip `Speed`/`Looping`/blending from the animation sets. Smooth rotations are spherical, not
  smoothed.
- Shading modes (`flat`, `standard`, `fullbright`, `reflective`) are read but not mapped; `unwrapMode` is ignored
  (every mode carries a layout); per-face `transparent` only chooses the material variant; `lockUVs` ignored.
- Biome tinting (grass and leaves are the greyscale textures), block randomness (`RandomRotation`, weighted
  textures: the first is used), `MinScale`/`MaxScale`, block animations (`CustomModelAnimation`), fluids,
  entities and block `components` in prefabs.
- Cube side textures read `North` as -Z and `South` as +Z; their UV orientation on top and bottom faces is the
  face's own, unrotated. Only 13 items give distinct side textures.
- Block `rotation`: the reading `yaw + 4 * pitch` is inferred from the values. **Yaw is verified** against real
  data (roofs climb towards -Z, -X, +Z, +X for 0 to 3, tested in `tests/game_data.rs`). The pitch and roll
  directions could not be checked (symmetric blocks only, or no roll values at all) and use the same
  right-handed sign; `bridge::ROTATION_SIGNS` flips them in one place if they prove wrong.
- The meaning of `filler` offsets is not decoded; only "does not draw again" is used.
- The server (`HytaleServer.jar`) is not touched.

## Legal

This repository publishes only code that reads files you already have: no game files, models, textures or
renders are included (the render scripts are ours; their output is game content and stays out). What the
official documents say, as read on 2026-10-23 (summarised, not legal advice):

- [EULA v2.3](https://hytale.com/eula) (effective 2026-01-13): the game is licensed, not sold, and Hypixel
  Studios Canada or its licensors own the game, art, audio and content (section 8.1). Mods, plugins, maps, skins
  and models may be created and distributed (3.1) and monetised under conditions, but you may not distribute the
  game or any part of it, including its files (3.2, 3.3), nor imply that a mod is official (3.2). Reverse
  engineering and decompiling are barred except as the law permits (4.1), with an interoperability carve-out that
  repeats the law's limits (4.2). "Hytale" may be used descriptively, not as a logo or endorsement (3.6).
- [Terms of Service v2.2](https://hytale.com/terms-of-service): the same reverse-engineering clause (4.2(a)),
  a bar on cheats, bots, hacks, exploits "or unauthorized third-party tools" (4.2(b), which does not define
  "unauthorized"), a bar on systematically collecting content from the Service to build collections (7.2(i), about
  the online service rather than local files), and a statement that mods are governed by the EULA (7.3).
- [Community Guidelines v1.0](https://hytale.com/community-guidelines) and
  [Server Operator Policies](https://hytale.com/server-policies): rules for servers and mods (no malware,
  respect others' IP).
- The studio's own [modding post](https://hytale.com/news/2025/11/hytale-modding-strategy-and-status) says
  modding is host-based, recommends Blockbench with its plugin for models and animations, and that the
  server is not obfuscated and can be decompiled until source is released.

Not found in these official pages: any rule that speaks to third-party tools that read the installed asset
files, or any fan-content or asset-use policy. This crate therefore ships no game data, claims no
affiliation, and leaves the legality of reading your own install to you.

## Licence

MIT or Apache-2.0, at your option ([LICENSE-MIT](LICENSE-MIT), [LICENSE-APACHE](LICENSE-APACHE)).

[JannisX11/hytale-blockbench-plugin]: https://github.com/JannisX11/hytale-blockbench-plugin
