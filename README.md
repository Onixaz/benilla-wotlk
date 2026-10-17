<p align="center">
  <img src="docs/assets/benilla-wotlk-banner.png" alt="Benilla WotLK" width="100%">
</p>

# Benilla-Wotlk

An open-source World of Warcraft 3.3.5a client built with Rust and Bevy.

Benilla-WotLK is an effort to preserve and recreate the original Wrath of the Lich King client experience using a modern, independently developed game engine.

Originally based on the [Benilla](https://github.com/samwhosung/benilla) project, which targets World of Warcraft 1.12.1, Benilla-WotLK has since evolved into a standalone project focused exclusively on version 3.3.5a (build 12340).

The client connects to compatible 3.3.5a servers using the original network protocol, loads game assets from your own WotLK installation, and runs the original interface directly from those files. The rendering engine, file format support, networking, and UI runtime are independently implemented in Rust, with the original 3.3.5a client serving as the behavioral reference.

This project does not distribute Blizzard's original client code or game assets. Users must provide their own game data.

Benilla-WotLK is an independent, community-driven preservation project, developed out of passion for the original game and the desire to keep its legacy playable for years to come.

## Status

> [!IMPORTANT]
> The 3.3.5a port is in progress. The client can authenticate, enter the world, and run the stock
> 3.3.5a interface, but parts of the data and gameplay layers still use 1.12.1 layouts or behavior.

Other client versions are not planned. Warden will come later. Use a 3.3.5a server with
Warden disabled for now.

## Highlights

- **Formats:** readers for the MPQ patch chain, BLP, DBC, ADT, WDT, WDL, M2, and WMO, exposed to
  Bevy as an asset source.
- **World:** streamed terrain, portal-culled buildings with interior lighting, doodads, ground
  clutter, swimmable water, sky, weather, and reference-style day/night lighting, fog, and gamma.
- **Models:** GPU-skinned M2 models with animation, particles, ribbons, spell visuals, character
  customization, armor compositing, weapons, enchant glows, forms, stealth, and mounts.
- **Movement:** bidirectional networked movement, server-granted movement modes, camera collision,
  transports, and taxi flights.
- **Networking:** SRP6 authentication, world-session crypto, and an ECS object mirror covering
  movement, chat, combat, spells, groups, raids, quests, trade, mail, auctions, and battlegrounds.
- **Interface:** a FrameXML and Lua engine that runs the stock 3.3.5a interface and third-party
  addons from the player's installation and local configuration.
- **Audio:** music, ambience, and effects with selection and crossfade rules, interior and
  underwater transitions, and zone reverb.

The format readers and UI engine core are plain Rust without Bevy. The world renderer can also run
without the game attached. See [`docs/MAP.md`](docs/MAP.md) for the generated map of crates,
subsystems, instruments, and scripts.

## Requirements

- An English World of Warcraft 3.3.5a client installation (build 12340). Benilla reads it but never
  writes to it.
- A 3.3.5a server with Warden disabled. Development uses
  [AzerothCore](https://github.com/azerothcore/azerothcore-wotlk).
- Stable Rust and a C compiler because the vendored Lua runtime is built from source.
  - Windows: the MSVC build tools selected by the Rust installer.
  - Linux: ALSA and udev development packages, plus `pkg-config`.
  - macOS: Xcode Command Line Tools.

## Quick start

Clone the repository, enter its directory, and point `WOW_DATA` at the `Data` directory of your
3.3.5a installation.

### Windows PowerShell

```powershell
git clone https://github.com/Onixaz/benilla-wotlk.git
Set-Location benilla-wotlk
$env:WOW_DATA = "C:\Games\World of Warcraft 3.3.5a\Data"
cargo run --release -p benilla
```

### Linux and macOS

```sh
git clone https://github.com/Onixaz/benilla-wotlk.git
cd benilla-wotlk
WOW_DATA=/path/to/WoW/Data cargo run --release -p benilla
```

Instead of setting `WOW_DATA`, you can create a `WoW` link at the repository root that points to
the installation directory:

```powershell
New-Item -ItemType Junction -Path WoW -Target "C:\Games\World of Warcraft 3.3.5a"
```

```sh
ln -s /path/to/WoW WoW
```

## Configuration

| Setting | Purpose |
| --- | --- |
| `WOW_DATA` | Path to the installation's `Data` directory. |
| `WOW_HOST` | Authentication endpoint; defaults to `localhost:3724`. A hostname or `host:port` is accepted. |
| `WOW_USER` and `WOW_PASS` | Optional credentials for automatic login. Set both or use the login screen. |
| `WOW_CHAR` | Optional character name for automatic selection. |

The Realmlist button on the login screen can also set and remember the server endpoint. Player
settings, screenshots, logs, and addons live under `benilla-config/` at the repository root. Place
addons in `benilla-config/AddOns/`.

## Documentation

- [`docs/METHOD.md`](docs/METHOD.md) defines the reference-first development method and safety
  rules.
- [`docs/MAP.md`](docs/MAP.md) is the generated map of what is currently implemented.
- [`docs/CONTRIBUTING.md`](docs/CONTRIBUTING.md) covers development setup, validation, and pull
  requests.

Issues and pull requests are welcome. Bugs, questions, and ideas can also be discussed on
[Discord](https://discord.gg/wJSJx467G4).

## Acknowledgements and legal

The project builds upon the original [Benilla](https://github.com/samwhosung/benilla) client and acknowledges the work of its original author and contributors.

Benilla-Wotlk is an independent fan project and is not affiliated with or endorsed by Blizzard
Entertainment. It ships no assets extracted from World of Warcraft: no models, sounds, maps, MPQ
contents, or FrameXML. You must provide your own legally obtained 3.3.5a installation. The files
under `crates/benilla-app/assets/ui/` are project-owned adapters and interface elements, not copies
from the game.

World of Warcraft is a trademark of Blizzard Entertainment, Inc. Benilla-Wotlk code is available under
your choice of the [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE) license. Vendored components
under `third_party/`, including the Kira audio engine and Lua 5.1, retain their own licenses.
