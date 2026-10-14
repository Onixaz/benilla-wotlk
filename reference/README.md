# `reference/`: verified facts about the reference client, vendored

Generated tables describing the 3.3.5a reference client, committed so benilla can measure itself
against them on any machine without re-deriving them. A filename begins with the client version it
describes. Data, not documentation: each generated surface has a generator beside it in
`scripts/`. `3.3.5a-message-catalog.tsv` also feeds `scripts/gen-message-catalog.py`, which
re-shapes it into `crates/benilla-ui/src/messages/catalog.rs`.

Nothing here is Blizzard content. A catalogue of names is not the thing named, the same way a
comment may quote a byte address and `docs/MAP.md` may name a file.

## 3.3.5a build 12340

`scripts/gen-reference-wotlk.py` accepts only the verified enUS build-12340 `Wow.exe` with SHA-256
`aa63a5750d60ef16746c686b3d5e26876d98953eab08b1c026cd0faf78e88cb8`. It reads that executable and
GNU objdump's instruction boundaries; it never copies executable bytes into this repository. Run:

```text
python scripts/gen-reference-wotlk.py --exe PATH/TO/Wow.exe --objdump PATH/TO/objdump
```

It regenerates eight executable-derived surfaces:

- `3.3.5a-event-catalog.tsv`: the exact 646 named entries in the 722-slot in-world event registry.
  The separate 41-slot GlueXML registry is also decoded and validated so its numeric ids cannot be
  mislabelled with world names.
- `3.3.5a-event-firesites.tsv`: every one of the 1,082 direct calls to the two event-dispatch
  wrappers. A computed value or an argument push across a control-flow boundary is `dyn`/`?`, never
  a guess. `3.3.5a-events.tsv` aggregates that evidence with the verified unit-field bridge and
  unit-token fan-out families, attributing a producer to 473 of 606 named events. It remains
  advisory because other computed-id paths may remain.
- `3.3.5a-shapes.tsv`: all 2,987 bindings in 113 exact global, widget-method, base-library and
  standard-library registrar tables. The registry facts are complete. Conservative control-flow
  and helper-return analysis proves return arity for 2,828 rows (2,191 exact and 637 set-valued),
  leaving 159 explicitly reasoned unknowns. Every row has a non-gateable observed argc floor, and
  2,445 rows have a matching return-kind stack trace. A `delegated-push` trace describes only the
  binding's visible suffix and remains advisory; only `kinds_conf=agree` without that note is fit
  for a kind gate.
- `3.3.5a-message-catalog.tsv`: all 730 rows of the `CGGameUI` message table at `0xac8af0`, including
  each key, display kind, sound, type tag and chat type.
- `3.3.5a-opcodes.tsv`: every literal incoming world-opcode registration in the exact executable,
  preserving opcode id, registration site, handler and context plus any directly fired FrameScript
  events. Symbolic names remain blank unless handler behaviour proves them; the seven
  `BATTLEFIELD_MGR_*` disagreements in published community tables are settled by their handlers'
  exact event fire sites.
- `3.3.5a-verb-events.tsv`: the 61 verb/event pairs whose dispatcher site lies in the registered
  verb's own body. It is explicitly a floor: helper attribution waits for verified build-12340
  function extents and a complete call graph.
- `3.3.5a-dbc.tsv`: all 241 DBC tables the client loads, one row per table loader (237 in the
  startup run at `0x6337d0`, four loaded on demand), with the column count and row size each
  loader checks, its table object, and the functions that read it. Readers are a floor (a read
  through a computed pointer is not seen), complete for the twelve `gt` tables, and attributed to
  Lua bindings and opcode handlers that read directly. Ten tables have no reader: 12340 loads
  them and never reads them.

The ninth surface, `3.3.5a-globals.tsv`, is the entire addon-visible global namespace: 46,312
names, `name<TAB>type<TAB>origin`. It unions 43,708 names captured in-world after loading all 23
shipped `Blizzard_*` addons (minus the capture addon's two globals) with 2,606 shipped-UI
definitions absent from that session as type `lod`. Its origin totals are 35 Lua 5.1, 2,079 engine
and 44,198 FrameXML names.

That table cannot come from the executable registry alone. The capture addon at
`scripts/reference-global-capture/` loads the shipped addons and records a sorted `_G`; generate its
union with the shipped UI using:

```text
python scripts/gen-reference-wotlk-globals.py --capture PATH/TO/BenillaReferenceCapture.lua \
    --data PATH/TO/WOW/Data --mpqcat PATH/TO/mpqcat
```

The generator refuses a capture with any unloaded shipped addon, validates the exact four enUS UI
MPQs by SHA-256, follows the 370-file live TOC/XML include graph, adds absent-session shipped
definitions as `lod`, and removes its own `BenillaReferenceCapture` and
`SLASH_BENILLAREFERENCECAPTURE1` globals.

## The gates

`crates/benilla-ui/src/script/tests/reference_surface.rs` fails if the engine puts a global
3.3.5a does not have in a bare VM and nobody wrote down why, with no prefix excusing an engine name.
`crates/benilla-app/src/ui_script/surface_gate.rs` fails if the production load, the core and
benilla's layer, adds any global past 3.3.5a other than the layer's own `Benilla`-prefixed names,
or any widget method. The shape, event and verb-event gates (`ui_script/shape_gate.rs`,
`event_shape_gate.rs`, `verb_event_gate.rs`) read the 3.3.5a tables but still apply 1.12.1's
rules, and are ignored until they are ported.
`crates/benilla-formats/tests/dbc_coverage.rs` fails if a table benilla reads is not one 12340
loads at the width its loader checks, if a loader reads a table `dbc_tables.rs` does not
register, if a table 12340 reads is neither read nor listed with what it backs, or if benilla
reads one of the tables 12340 never reads without saying so.
