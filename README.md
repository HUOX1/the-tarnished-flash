# The Tarnished Flash — native plugin source

ModEngine3 native plugin for **ELDEN RING 1.17.2**.

When triggered, it temporarily multiplies the speed field of every character
within a radius around the player, then restores the original values when the
effect ends. Nothing else in the game is modified.

This repository contains the plugin source **only**. The item that triggers the
effect (a custom consumable) lives in the game files and is not part of this
code base.

## What it touches

* Reads `CS::WorldChrMan` through the game's own static pointer and validates its
  vtable against the build before doing anything. A wrong build is refused with
  `GAME_VERSION_MISMATCH` instead of writing into unknown offsets.
* Walks the three character arrays, screens candidates by vtable, `chr_type`,
  container and owner back-references, then writes **one float** per target:
  `behavior + 0x17C8` (speed multiplier).
* Stores the original value per target and writes it back on timeout.
* Nothing is written before the target's identity has been verified, and a
  target whose identity cannot be re-verified is never written a second time.

## Trigger and keys

| Key | Action |
| --- | --- |
| `F8` | Debug trigger (only when `DebugHotkeyEnabled=1`) |
| `F9` | Manual sweep: restores every address this session wrote that still holds our scale |
| `F10` | Diagnostic: prints every candidate character array with its nearest character |

The normal trigger is a custom consumable; it is detected by watching one
`SpEffect` id on the player.

## Config (`SpeedForce.ini`, next to the DLL)

| Section | Key | Meaning |
| --- | --- | --- |
| General | `Enabled` | 1 = armed |
| General | `DebugLog` | 1 = verbose diagnostics |
| SlowMotion | `Radius` | metres, default 80 |
| SlowMotion | `SlowScale` | speed multiplier while active, default 0.2 |
| SlowMotion | `DurationMs` | effect length, default 15000 |
| SlowMotion | `MaxTargets` | hard cap, default 128 |
| SlowMotion | `TargetLimit` | 0 = no extra limit |
| SlowMotion | `DebugHotkeyEnabled` | enables F8 |
| Trigger | `EnableItemTrigger` | enables the consumable trigger |
| Trigger | `SignalSpEffectID` | SpEffect id watched on the player |
| Trigger | `PollIntervalMs` | polling interval, default 10 ms |
| Trigger | `ItemTriggerDryRun` | log-only mode for the detector |

## Known limitations

* **Character streaming.** Right after loading the game, respawning, or entering
  a new area, the game has not populated its character list yet. The plugin
  retries the scan and keeps re-scanning while the effect runs, but if you
  activate too early the effect may only cover part of the duration. Wait about
  20 seconds after a load.
* **Magic projectiles can vanish mid-air.** Projectiles are slowed, but magic
  bullets have a fixed lifetime, so a slowed spell may expire before reaching
  you. Arrows do not appear to behave this way.
* **The consumable is consumed while the effect is active.** Using it during an
  active window does nothing and does not refund the item.
* A target that loses its identity mid-effect is dropped from the list, and the
  plugin tries to re-derive the address from the character itself at the end. If
  that also fails, the value is left untouched; `F9` cleans up anything left
  behind.

## Safety

* No network access of any kind.
* No telemetry.
* No DRM or anti-cheat interaction.
* The only files written are local debug logs under `natives/Logs/`
  (`DebugLog=0` disables them).
* No third-party crates: `[dependencies]` is empty on purpose, so the whole
  plugin can be audited in one pass.

## Building

See [BUILD.md](BUILD.md).

## Licence

MIT, see [LICENSE](LICENSE).
