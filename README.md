<div align="center">

```text
█████    █  █  █  █████  █████  ████   █████  █████ 
█        █  █  █  █      █      █   █    █      █   
████     █████  █  ████   ████   █   █    █      █  
    █    █  █   █ █      █      █   █    █      █   
█████    █  █    █ █████  █████  ████   █████    █  
```

**saveedit — character save editor for Romestead**

**softcore toggle · gold · full item editor · searchable 700-item game database**

CLI + interactive TUI in a single dependency-free Rust binary.
Patches bytes directly in the `CandideSerializer` format — no save *re-writing*, no corruption.

[![Release](https://img.shields.io/github/v/release/Pitu7944/romestead-save-editor?include_prereleases&logo=github&style=flat-square)](https://github.com/Pitu7944/romestead-save-editor/releases)
[![License](https://img.shields.io/github/license/Pitu7944/romestead-save-editor?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-dea584?logo=rust&style=flat-square)](https://www.rust-lang.org)
[![Platform](https://img.shields.io/badge/platform-Windows%2010%2F11-blue?logo=windows&style=flat-square)]()
[![Saves are backed up](https://img.shields.io/badge/saves-always%20backed%20up-success?logo=shield&style=flat-square)]()
[![PRs welcome](https://img.shields.io/badge/PRs-welcome-brightgreen?style=flat-square)]()

</div>

> [!WARNING]
> Fan-made tool, not affiliated with the Romestead devs. Editing saves is at your own risk —
> though this tool was built to make that risk about as close to zero as it gets
> (see [Safety & reliability](#safety--reliability)). If the game has online leaderboards or
> shared characters, edited saves may get you flagged.

---

| | |
|---|---|
| **[Screenshots](#screenshots)** | gallery below — save picker, item browser, picker, CLI |
| **[Quick start](#quick-start)** | build it in one command, run it with one more |
| **[Item database](#item-database-700-items)** | 700 in-game items with names, rarities, damage types, bonuses and prices |
| **[Safety](#safety--reliability)** | byte-level `.bak` backups, post-edit verification, width-change guards |
| **[Format reference](#format-reference)** | how Romestead `.char` saves actually work |

## Contents

- [Features](#features)
- [Quick start](#quick-start)
- [CLI usage](#cli-usage)
- [Interactive TUI](#interactive-tui)
- [Item database (700 items)](#item-database-700-items)
- [Batch & housekeeping](#batch--housekeeping)
- [Safety & reliability](#safety--reliability)
- [Format reference](#format-reference)
- [How it was built](#how-it-was-built)
- [FAQ](#faq)
- [Screenshots](#screenshots)
- [License](#license)

## Features

| | Feature | What you get |
|---|---|---|
| **S** | **Single exe, zero deps** | one `saveedit.exe`, no runtime, no DLLs — `cargo build --release` |
| **A** | **Always safe** | every edit round-trip verified by re-parsing before anything is written; `file.bak` always written first |
| **V** | **Visual TUI** | searchable item picker with game names, rarity colors, damage stats and a details pane; mouse + undo included |
| **E** | **Entity of knowledge** | all 700 items extracted from the game code: id, name, rarity/tier, damage types, stat bonuses, price, usable info |
| **E** | **Easy batching** | a manifest gives you many items in one shot; `merge` cleans up duplicate stacks; `sortinv` ranks your inventory |
| **D** | **Documented format** | full `.char` (CandideSerializer) layout reference below, plus a readable single-file Lua reference edition |

## Quick start

```sh
git clone https://github.com/Pitu7944/romestead-save-editor
cd romestead-save-editor/saveedit-rs
cargo build --release          # -> target/release/saveedit.exe

# enable softcore on a character
./target/release/saveedit.exe difficulty Marcella.char softcore
```

Any Rust 1.75+. No external binaries needed. Done in under a minute on a modern machine.

## CLI usage

```sh
saveedit dump       <file>                          # show header, difficulty, money
saveedit difficulty <file> <softcore|normal> [out]  # toggle difficulty
saveedit money      <file> <amount>           [out]  # set gold (inserts missing field)
saveedit items      <file> [inventory|equipment|secondary] [--json]
saveedit give       <file> <itemId> [count] [target] [slot] [out]
saveedit give-file  <file> <manifest.txt>     [out]  # many items, ONE backup+write
saveedit remove     <file> <slot> [target]    [out]
saveedit stack      <file> <slot> <count> [target] [out]
saveedit merge      <file> [target]           [out]  # consolidate duplicate stacks
saveedit mergeall   <file>                    [out]  # merge across all 3 arrays
saveedit sortinv    <file>                    [out]  # inventory ranked by rarity
saveedit db         [filter] [--json]                # search the 700-item database
saveedit tui        [folder]                         # the whole thing, interactive
```

- With no `outfile` the file is edited **in place** — and `file.bak` of the *original* is
  always written first. Give an `outfile` to leave the input untouched (a `.bak` is still
  produced next to it).
- `give` places into the first empty slot (`inventory` by default); pass an explicit
  1-based `slot` to place elsewhere (only empty slots are used).
- Item ids are like `weapon:water:sword`, `food:crab_meat`, `material:gold_ore` — browsing
  `saveedit db` shows every valid id. A typo costs nothing: unknown ids show up in-game as an
  "Invalid id" item instead of crashing.

### Examples

```sh
# give 25 gold ore, in place (backup made first)
saveedit give Marcella.char material:gold_ore 25

# stack up something in SecondaryEquipment slot 4
saveedit stack Adolfina.char 4 99 secondary

# merge duplicate stacks everywhere, then rank the inventory
saveedit mergeall Adolfina.char
saveedit sortinv Adolfina.char

# or just open the interactive editor
saveedit tui .
```

## Interactive TUI

`saveedit tui [folder]` walks the same flows with keyboard and mouse:

```text
┌─ save picker ──┐   ┌─ edit menu ──┐   ┌─ item browser ─┐   ┌─ detail pane ─┐
│ Adolfina   …   │ → │ difficulty   │ → │ arrows: slots  │ → │ dmg, bonuses, │
│ Marcella   …   │   │ money        │   │ g: give item   │   │ price, flavor │
│ Spejson    …   │   │ inventory    │   │ r: remove      │   │ text          │
└────────────────┘   └──────────────┘   └────────────────┘   └───────────────┘
```

| Key | Action |
|---|---|
| `↑↓` / click / wheel | move through saves, menu entries and slots |
| `enter` / click again | select / open |
| `g` on an empty slot | open the **item picker** (the full game DB) |
| `r` / `c` | remove item / set stack count of the slot under the cursor |
| `esc` | back one screen · `q` quit from the save picker |
| `ctrl+z` | **undo** your last edit in this session (refuses if the game touched the file meanwhile) |
| (picker) `Tab` | toggle sort: rarity (legendary first) ↔ name |
| (picker) `F2` / `F3` | favorite the selection / cycle category (weapon, food, material, …) |
| (picker) `type` | live filter by **id or game name** |

- Recently-given and favorited items float to the top and persist per folder in
  `.saveedit_recents.json` (delete the file to reset).
- Every action lands instantly (backup → edit → verify → redraw) and the footer says what happened.
- Every screen keeps a `Name · difficulty · money` summary bar in the header.

## Item database (700 items)

The binary embeds the game's full item database — id, name, rarity
(tier 0-6: trash, common, uncommon, rare, epic, legendary, exceptional), stats
(weapon damage **per damage type**, stat bonuses, shield stats), price and usable info:

```sh
saveedit db                    # list all 700 items
saveedit db pickaxe            # filter by id or name (whitespace-separated tokens)
saveedit db "water sword"      # id:weapon:water:sword  Gladius Aquaticus  uncommon
```

Each row shows id, name, rarity, tier, icon and a stats summary:

```text
weapon:water:sword   Gladius Aquaticus  uncommon  2  sword:blue  Weapon · weapon sword · dmg [Aqua 10-12] · sw 0.4 · st 0.25 · 250 gold
armor:camillus:chest Camillus Robes     common    1  caster:chest:camillus  Armor · Armor +5 · Health +4 · 50 gold
```

Damage comes with proper damage-type names straight from the game's own enum,
from decompiled `Shared.Combat.DamageTypes`:

```text
True · Slashing · Piercing · Bludgeoning · Pyro · Chloro · Aqua · Cosmo · Necro
```

## Batch & housekeeping

- `give-file` applies a manifest — one `<itemId> [count] [target]` per line, `#` comments —
  giving many items with **one** backup and one write.

  ```text
  # starter-kit.txt
  weapon:gold:sword 1 equipment
  material:gold_ore 25
  food:crab_meat 10
  ```

- `merge` consolidates duplicate stacks: the earliest slot fills up to the game's
  MaxStackSize (from the item DB), duplicates are emptied. `mergeall` covers
  Inventory + both equipment arrays.
- `sortinv` reorders Inventory by rarity (legendary first, then id) and moves empty slots to
  the end — a pure byte permutation of same-length elements, nothing else in the file moves.
- `--json` on `items`/`db` prints machine-readable JSON (rarity, named damage ranges, …)
  for scripting and diffing saves.

## Safety & reliability

This tool was written by **diffing real saves against decompiled game code**, and it treats
saves as hostile input:

- **Always backed up**: `file.bak` (of the *original*) is written before anything is touched.
- **Never blind**: every patch is fully re-parsed and asserted before the output file is written.
- **Width-change guards**: if any container's length encoding would change byte-width, the edit
  is refused with a clear error instead of corrupting the file — this protects saves from game
  versions the tool doesn't understand.
- **Idempotent**: re-running a command that's already satisfied prints "already ..." and does nothing.

Only compact-strings saves (`ceb\x01` magic — what the current game writes) are supported.

## Format reference

```text
"ceb\x01"                      magic + string-map flag
[7bit count][0x21][count x raw strings]   string map; string id = 1-based index
[0x20][7bit numFields][7bit totalLen][entries]   a (compact) object; entries follow consecutively
[typecode][7bit nameId][7bit payloadLen][payload]   one field entry
```

Payload layouts used by this tool:

| Field | Typecode | Payload |
|---|---|---|
| `Difficulty` (in CharacterModel) | `38` NullableEnum | `[3 (Int32)][int32 LE]`, 0 = Normal, 1 = Softcore |
| `Money` (root) | `20` UInt64 | 8 bytes LE |
| object | `32` CompactObject | as object layout above |
| string (compact) | `34` | `7bit stringId` |
| Int32 | `3` | 4 bytes LE |
| Guid | `37` | 16 bytes |
| array | `49` | `[7bit count][0x21 elemTc][elements]` |
| interface/array-of-nulls | `48` | null = `[0][0]`; value = `[7bit asmId][7bit typeId][typecode][value]` |

Item instances (`Shared.Models.Items.ItemInstanceModel`) serialize these fields in declaration
order: `BaseDataId` (compact string), `Id` (Guid), `InventoryId` (Guid via typecode 48),
`Auras` (array, serialized even when empty), `StackCount` (Int32), and optionally `UsableInfo`.

Crucially, **fields whose value equals the type default are omitted entirely**
(`GenericTypeHandler.Serialize` skips them). That's why a Normal save has *no* `Difficulty`
field at all, and a `Money` field only appears once you have non-zero cash. To enable something
that is absent, the tool inserts a properly formed entry and grows the correct enclosing length
headers. 7-bit integers are standard LEB128 (least-significant group first, high bit = continuation).

## How it was built

1. Diffed a known-Softcore against a Normal save → `Difficulty` present vs omitted.
2. Decompiled the serializer/deserializer from `CandideCreator.Shared.dll` with
   [ILSpy](https://github.com/icsharpcode/ILSpy) (`ilspycmd`).
3. Wrote a parser/patcher first in Lua, verified every byte region it changes, then ported to
   Rust with a [ratatui](https://github.com/ratatui/ratatui) TUI.
4. Extracted the item database from decompiled `Shared.Data.Items.SharedDataSetup.cs` with
   `saveedit-rs/tools/extract_items.py` and embedded it into the binary.
5. The Rust output is verified byte-identical to the Lua reference for the same operations.

## FAQ

**Is this safe? Will the game accept an edited file?**
Yes — the game parses its saves leniently, and this tool only writes structures it understands,
with backups and verification (see [Safety](#safety--reliability)).

**Can I give myself an item that doesn't exist, or 2^64 gold?**
You can write any id (it shows as "Invalid id" in-game) and any u64 gold value; the game clamps
or ignores what it doesn't like. The `db` command is there to keep you honest.

**Why "give" at a specific slot?**
Because inventory is a fixed 36-slot array in the save. `give` only ever uses *empty* slots,
so your existing items are never touched.

**Can consumables be made usable?**
Yes. When gifting, the tool synthesizes the `UsableInfo` sub-object the game derives at pickup
(from `ItemInstanceModel.CalculateStats`) — so gifted food and drinks are fully consumable, not
just stackable/sellable.

**Does it work on saves from a different game version?**
It refuses (with a clear error) rather than guessing: any container whose length encoding
width would change aborts the edit. Older/newer versions may need format updates — PRs welcome.

## Screenshots

| | |
|---|---|
| ![save picker](docs/screenshots/tui-saves.png) | `saveedit tui .` — the save picker with the summary bar |
| ![edit menu](docs/screenshots/tui-menu.png) | edit menu + character summary header |
| ![item browser](docs/screenshots/tui-items.png) | inventory browser: rarity colors, game names, damage |
| ![browser + details](docs/screenshots/tui-detail.png) | detail pane with bonuses / price |
| ![item picker](docs/screenshots/tui-picker.png) | picker filtered to "sword", legendary first |
| ![picker details](docs/screenshots/tui-picker-detail.png) | picker detail pane with damage types |
| ![cli db](docs/screenshots/cli-db.png) | `saveedit db pickaxe` |
| ![cli items](docs/screenshots/cli-items.png) | `saveedit items` with rarity colors |
| ![cli give-file](docs/screenshots/cli-givefile.png) | `saveedit give-file` applying a manifest |

## License

MIT — see [LICENSE](LICENSE). Romestead and any referenced item names belong to the game's
authors; this repository is a community tool and is not affiliated with or endorsed by them.