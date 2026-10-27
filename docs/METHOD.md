# METHOD — how benilla is built

benilla is a from-scratch World of Warcraft 3.3.5a (build 12340) client in Rust and Bevy. It
reads the game's data from the player's own install, speaks the original protocol to any 3.3.5a
server, and does not build a server or simulate the game. The reference client is the spec:
benilla does what 3.3.5a does, as a modern, idiomatic client.

## The rule for every change

A change is right when it makes benilla more like 3.3.5a or fixes a bug, with evidence from the
reference, in one small piece, with the gates green. A deviation from the reference is the
maintainer's call alone, and each one is written down where it lives: a CVar's `Deviates` row, a
comment naming the reference fact and why we differ. Anything else lives on top of benilla, in a
crate of its own or a fork, and benilla opens a piece of itself to such a crate only when its own
3.3.5a work creates that piece (`docs/CONTRIBUTING.md`, "Building on top").

## The loop

1. **The reference first.** For anything fidelity-defining, get the mechanism from the real
   client before building: its behaviour, a DBC field, a FrameXML line, a packet capture. The
   names and shapes under `reference/` are the surface benilla tracks.
2. **Build it idiomatically.** Modern Bevy and Rust, performance woven in from the start.
   Implement the mechanism well; do not ape a quirk that is a bug.
3. **Measure, never eyeball.** Timing, delay, ordering and feel are settled with instruments:
   trace timestamps, byte-derived numbers, a numeric probe. A capture confirms an existence
   fact; it cannot measure time. Never search for a viewpoint: take the one simplest shot, and
   if it does not settle the question, hand over a precise script ("log in, look at X, does Y?").
   Frame time is measured, and every performance A/B run, at Dalaran (map 571, 5842.4 595.6
   650.7), the busiest place in the game.
4. **Close the loop on the symptom.** A bug reported as seen is closed by reproducing it at the
   reported spot first, fixing, and showing the same instrument clean there plus one adjacent
   state. Green gates and an existing mechanism are never that evidence.
5. **Prove the run before reading the result.** A live result is evidence once the run is valid
   from numbers: preflight banner clean, camera and body where intended, the subject in frame,
   the window long enough. A negative from an unproven run is not evidence.
6. **Commit small and often; land by pull request.** Atomic commits, explicit paths, on a
   branch; main moves only by a squash-merged pull request, one commit per piece of work.
   No "done" and no commit without saying what was verified and how the result was judged.

## Hard rules

- **Never commit anything from the install.** Art, models, sounds, maps, data: the install is
  read at runtime from a gitignored path, and everyone provides their own. Interface code is no
  exception: FrameXML and GlueXML run off the player's own patch chain, and `assets/ui` holds
  only benilla's own files.
- **The install is read-only, but for the drop-in.** No build writes into the WoW folder (no
  screenshot, log, cache or scratch file; `scripts/smoke.sh` fails a run that leaves it changed)
  except the `standalone` build, which replaces `Wow.exe` in the install root and keeps its
  state there as 3.3.5a does, so the two clients take turns on one folder. Only that build flag
  opens the install for writing, never the folder a binary runs from.
- **Local state is the reference's own files.** What 3.3.5a writes, benilla writes in its
  format at its path (`WTF\`, `Cache\WDB\`, `Logs\`, `Screenshots\`, addons in
  `Interface\AddOns\`), under one root: the install for the drop-in, `benilla-config/` for every
  other build (at the repo root in a dev build, beside the binary in the player build). What
  3.3.5a has no file for stays in `benilla-config/` itself. Every path resolves through
  `crate::local_state`. Player settings are CVars, saved where 3.3.5a saves them.
- **The core UI is the stock UI; benilla's own is one layer on top.** The core loads the stock
  3.3.5a FrameXML off the player's own chain, byte for byte, and builds the engine verbs those
  files call, never stubbing one to make a file load. benilla's own interface (its options
  window, its game-menu look, its fixes to the stock Lua) is one layer that loads after the
  stock files and before third-party addons, the way an addon does: written against the 3.3.5a
  API alone, listed in no addon list and always on, and offering nothing another addon can build
  on.
  Anything that changes what the stock UI does or shows lives in the layer, and the layer does
  not grow: a test names its files and fails on a new one.
- **A setting's default is the reference's default.** Every option boots at the stock 3.3.5a
  value. Shipping another value costs an explicit `Deviates` row with the reason, and a test
  fails a row that drifts either way.
- **Our code is MIT OR Apache-2.0**, which is a claim about provenance: no original client
  code, no bundled assets.

## Where things live

- `docs/METHOD.md`, this file: rules and method. Never state, never a log.
- `docs/MAP.md`: what is built, generated from the code by `scripts/genmap.sh` at every land.
  Read it to orient; never edit or commit it by hand.
- git: the history. No changelogs, no status docs, no "what we did" narratives.
- The code: a comment says what the code does and the reference fact behind it, in a line. No
  history, no stories.

## Gates

- **Two builds.** `dev` (`cargo dev`) is optimized, incremental, with debug assertions and
  overflow checks on benilla's own crates. Every scripted run, probe, test and gate builds it, so
  they share one set of artifacts. `release` is built, never tested: `cargo ship` builds the
  shipped drop-in, the player build with `standalone`, to ship and to measure frame time from an
  install root, which the dev build never does. `cargo play` (without `dev`) runs `play`,
  `release` with thin LTO, to play on, with the PGO profile each machine records with
  `scripts/pgo-train.sh` when there is one (`release`, with `BENILLA_PGO_LTO=fat`).
- **The check**: `scripts/check.sh`, per round and before our own pull requests, on the whole
  workspace: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, the
  doc-link and render-pass lints, then `cargo test`, the player build without the `dev` feature
  with its own tests, and the engine boot, each against the install and the addon corpus and
  again with neither (`WOW_DATA=`, `BENILLA_ADDON_CORPUS=`), so a test that reads the install
  without opening with `wow_data_or_skip!` fails here and not on a clone. `CHECK_SCOPED=1`
  narrows it to the changed crates and their dependents. A test that reads data skips where it
  is absent and the check prints the count; where the data is, a skip is a failure
  (`BENILLA_REQUIRE_DATA=1`). Run it once per round; `tee` the output to a file and grep the
  file.
- **The land gates**: `scripts/gates.sh`, the check's chain without the data, for a pull request
  from an external contributor; CI runs it on a fork's pull request. It is memoized on the tree.
- **A clean run** is the fourth gate: `scripts/smoke.sh`, a live login and logout against the
  local server. For wire work the gate is a live run with a trace (`WOW_MOVE_TRACE=<path>`,
  tags filtered), never a capture: capture mode has no network.
- **Platform seams**: a `cfg(target_os)`, a `[target.'cfg(…)']` dependency, a `#[link]` or an
  `extern "system"` is invisible to the gates of the platform you are on. Say in the commit
  which platforms you built.

## The local server

benilla talks to any 3.3.5a server: `WOW_HOST` names it (default `localhost`), `WOW_USER` and
`WOW_PASS` together log in without typing, `WOW_CHAR` picks the character. There is no default
account. These bind every session and every agent:

- A scripted run logs in as an account its checkout declares in `.probe-identity` (the three
  variables, one per line, never committed; `[name]` sections declare more), or as the three
  variables name; never as a player's, because a login kicks whoever holds the account, and the
  client refuses a scripted login from a checkout on an account it does not declare, or on one
  another run on the machine holds. Parallel runs each take their own: `WOW_PROBE_AS=pool`.
- Every unattended run says so: `WOW_UNATTENDED=1`. An agent's run is silent: `WOW_NOSOUND=1`.
- A scripted run is the dev build (`cargo build -p benilla`, `target/debug/benilla.exe`);
  `release` links with fat LTO and is built only to ship, to measure frame time or when asked
  for.
- A combat probe keeps its character alive: `.cheat god on` before the fight.
- Anything about hostility, reaction colour, nameplates, threat, damage or breath runs with
  `WOW_GM=off`.
- Read the preflight banner before debugging anything else.

The switches for such a run are `docs/CONTRIBUTING.md`, "Running it unattended".
