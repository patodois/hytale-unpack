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

## Build and use

```sh
cargo build --release          # the `bridge` feature (Provider, glTF, model/prefab export) is on by default
cargo build --release --no-default-features   # parsers, `anim`, `prefab` (print only), `check`

H=hytale-unpack                # target/release/hytale-unpack
# Assets.zip is found at $HYTALE_ASSETS, else the macOS launcher's install; --assets <file> overrides

$H list Kweebec                # ids of models, entity models, block items and prefabs containing the text
$H list Kweebec --raw          # raw archive entries instead
$H model Human/Player -o out/  # entity model + its clips -> out/Player.gltf (+ .bin, .png)
$H model Server/Models/Intelligent/Trork/Trork.json --clips Walk -o out/
$H model Bench_WorkBench -o out/                  # a block item
$H model Common/NPC/Beast/Bear_Grizzly/Models/Model.blockymodel   # a bare model, print only
$H anim Walk                   # a clip: frames, nodes, keyframes (--keys lists them)
$H prefab Server/Prefabs/Npc/Kweebec/Autumn/Houses_Small/Kweebec_Autumn_Houses_Small_001.prefab.json
$H prefab <path> -o out/       # the same, plus the building as glTF with each block model stored once

$H check                       # parse every model, clip and prefab (streaming); counts and peak RSS
$H check-bridge --scenes 30    # load everything through the Provider and validate it (prefabs under 30 MB)
```

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

`hytale_unpack::bridge::Hytale` implements `rewrite_bridge::Provider` (rev `1b58201`). Conventions are the
bridge's: metres, +Y up, right-handed, +Z front, counter-clockwise front faces, sRGB textures. One block (the
game's unit) is taken as one metre, which the data does not state; it fits the player (hitbox 1.85 high,
eye height 1.6, model about 1.89 tall).

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
| `Server/Models/**.json` | Character | model + texture; clips from `AnimationSets` |
| `Common/**.blockymodel` | Character, Weapon, Prop, Other by folder | model + the texture found next to it; clips by folder rule |
| `Server/Item/Items/**.json` with a `BlockType` | Prop | the block: textured cube or custom model, origin at the bottom centre of its cell |
| `Server/Prefabs/**.prefab.json` | Level | `load_scene` only: one instance per block cell |

- **Hierarchy.** One bridge node per Hytale node (its pivot), named after it; a node with a visible shape has one
  child `<name>/shape` carrying the shape's offset and stretch, and the mesh (one per shape, one triangle list of
  quads). Shapes with `visible: false` have no mesh. Materials: the texture, matte (`metallic 0`, `roughness 1`),
  in variants for double-sided shapes and for shapes with a `transparent` face; a texture with transparent texels is
  alpha-masked at 0.5, a `transparent` face blends.
- **Which clips belong to a model** (`load_animated`):
  1. an entity model's `AnimationSets` (with `Parent` inheritance, so the Trork gets the player's sets unless it
     overrides them): a one-clip set is named after the set (`Walk`), a bigger one `Set/<file>`;
  2. for a bare model `<dir>/<name>.blockymodel`, the plugin's rule: every `.blockyanim` in each folder of
     `<dir>/../Animations/`, named `<folder>/<file>`.
  Clips address nodes by name and drive every node of that name; nodes the model lacks are skipped.
- **Clips become channels:** `position` -> translation (rest + delta, metres), `orientation` -> rotation (rest *
  delta, spherical), `shapeStretch` -> scale of the `<name>/shape` node (stretch * delta). `smooth` position and
  stretch keyframes become cubic-spline keyframes reproducing the plugin's Catmull-Rom preview; rotations are always
  spherical (the plugin does this for Hytale formats). Times are frames / 60.
- **Scenes** hold one inline model per distinct block and one instance per cell: cell `(x, y, z)` is
  `[x, x+1] x [y, y+1] x [z, z+1]`, moved by minus the anchor, with the anchor cell centred on the origin and its
  floor at y = 0. `rotation` is `yaw + 4 * pitch (+ 16 * roll)` in quarter turns, applied about the cell centre
  as `Ry(yaw) * Rx(pitch) * Rz(roll)`. Cells with a `filler` (parts of bigger blocks) and `Empty` cells are not drawn.
  `load_scene_report` also returns what was skipped.

## Rendering checks

`render/` has a three.js + Playwright script that renders an exported glTF headless (`npm install` there, then
`node shot.cjs out.png model.gltf "anim=Walk&frac=0.33&fit=1.0&dir=0.6,0.3,1" 800x700`). Used to check by eye
(exported glTF files and renders are game content and are not in this repository):

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
a keyframe past their `duration` (noted, not an error). **Peak RSS 79.9 MB, 6.5 s** (`/usr/bin/time -l`: 78 MB).

`check-bridge --scenes 30` loads all 436 entity models with a model (69,323 clips built), all 2,830 bare models,
all 2,956 block items and all 7,828 prefabs as scenes (14.75 million instances), validating each: 0 failures,
no unresolved block names. Peak RSS 262 MB (the 23 MB prefab dominates).

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
- The meaning of `filler` offsets is not decoded; only "does not draw again" is used. Cell `[x, x+1]` versus a
  cell centred on `x` is a convention (relative placement is unaffected).
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
- [Terms of Service v2.2](https://hytale.com/terms-of-service): the same reverse-engineering clause (4.2),
  a bar on systematically collecting content from the Service (about the online service, not local files), and a
  statement that mods are governed by the EULA.
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
