# gamingg — Linux build

A ready-to-run build of the game, for testing on a Steam Deck or any x86-64
Linux desktop.

```
gamingg-linux-x86_64    the game, stripped, ~9.7 MB
SHA256SUMS              so you can tell one build from the next
```

There is one binary here and it is overwritten each round rather than
accumulating — ten megabytes a build adds up fast in a git pack. If handing out
builds becomes routine this should move to GitHub Releases instead of living in
the tree.

---

## Check this first — thirty seconds

The build needs **glibc 2.39 or newer**. Rust's standard library pulls in
`pidfd_spawnp@GLIBC_2.39`, and that version requirement is not marked weak, so
the loader refuses the binary outright on anything older rather than degrading
gracefully.

```bash
ldd --version | head -1
```

- **SteamOS 3.7 and up** is Arch-based with glibc 2.41. Fine.
- **SteamOS 3.6** shipped glibc 2.37. This build will not start, and the error
  will be `version 'GLIBC_2.39' not found`.

If you are on the older one, that needs a rebuild against an older glibc — a
different build container, not something that can be patched here. Say so and it
gets done next round.

## Running it

```bash
# Deck, Desktop Mode, in Konsole:
git clone <this repo> && cd gamingg
./dist/gamingg-linux-x86_64
```

`git` preserves the executable bit, so a clone needs no `chmod`. A file
downloaded through a browser does: `chmod +x gamingg-linux-x86_64`.

To play it in Gaming Mode, add it as a Non-Steam Game.

**It is keyboard-and-mouse.** The Deck's built-in sticks, pads and triggers will
not drive it without a Steam Input keyboard layout, so the first run wants a USB
or Bluetooth keyboard in Desktop Mode. Gamepad bindings are not built yet.

## What it needs from the system

Almost nothing. Both shaders are compiled into the binary and there is no asset
directory, so this one file is the whole game. It links only `libc`, `libm` and
`libgcc_s`; X11, Wayland and Vulkan are all opened at runtime, so it uses
whatever the machine has. The Deck's AMD GPU runs the RADV Vulkan driver
natively and needs nothing installed.

Worlds and settings follow the XDG spec:

```
~/.local/share/gamingg/saves     worlds
~/.config/gamingg                settings
```

Both are under your home directory, which matters on SteamOS — the root
filesystem is read-only, so the game must never be run from a directory it
expects to write to.

## Useful flags

```bash
./gamingg-linux-x86_64 --help              every option
./gamingg-linux-x86_64 --seed 12345        a different world
./gamingg-linux-x86_64 --third-person      start over the shoulder
./gamingg-linux-x86_64 --screenshot out.ppm --at 40,40
```

## Controls

`WASD` move, `Space` jump, `Left Shift` crouch, `Left Ctrl` sprint,
`Left Ctrl`+`Left Shift` slide, `Z` prone, `C` first/third person, `E` interact,
`V` handheld, `M` mark ore, `Tab` cycle mining method. Waist-high ledges are
vaulted automatically — you do not press anything for those.

The full list is in the main `README.md`.

## If it does not start

| What you see | What it is |
|---|---|
| `version 'GLIBC_2.39' not found` | SteamOS too old — see the check at the top |
| `Permission denied` | `chmod +x gamingg-linux-x86_64` |
| No window, Vulkan errors | The GPU driver is not being found; run it from Desktop Mode rather than through Gaming Mode's overlay |

## Verifying the download

```bash
sha256sum -c SHA256SUMS
```
