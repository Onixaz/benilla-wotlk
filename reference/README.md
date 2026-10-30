# `reference/`: verified facts about the reference client, vendored

Tables describing the 3.3.5a reference client, committed so benilla can measure itself against them
on any machine without re-deriving them. A filename begins with the client version it describes.
Data, not documentation. `3.3.5a-message-catalog.tsv` also feeds `scripts/gen-message-catalog.py`,
which re-shapes it into `crates/benilla-ui/src/messages/catalog.rs`.

Nothing here is Blizzard content: a catalogue of names is not the thing named, the same way
`docs/MAP.md` may name a file. The tables carry no client addresses and no names of the client's
internal code; a table the client registers is named by an opaque id.

## 3.3.5a build 12340

Eight tables were derived from the enUS build-12340 client and are vendored as they stand:

- `3.3.5a-event-catalog.tsv`: the 646 named entries of the 722-slot in-world event table.
- `3.3.5a-glue-events.tsv`: the 41-slot glue-screen event table, which the glue screens load in
  place of the in-world one, so a glue event's id names a different event in-game.
- `3.3.5a-events.tsv`: each named event with its argument formats and producer families,
  attributing a producer to 473 of 606 named events. It remains advisory because computed-id
  paths may remain.
- `3.3.5a-shapes.tsv`: all 2,987 bindings in 113 global, widget-method, base-library and
  standard-library registrar tables (`table` is the table's id, `t001`..`t113` in client order).
  Return arity is proven for 2,828 rows (2,191 exact and 637 set-valued), leaving 159 explicitly
  reasoned unknowns. Every row has a non-gateable observed argc floor, and 2,445 rows have a
  matching return-kind trace. A `delegated-push` trace describes only the binding's visible suffix
  and remains advisory; only `kinds_conf=agree` without that note is fit for a kind gate.
- `3.3.5a-message-catalog.tsv`: all 730 rows of the UI message table, each key with its display
  kind, sound, type tag and chat type.
- `3.3.5a-opcodes.tsv`: every incoming world opcode the client registers a handler for, with the
  events its handler fires directly. Symbolic names remain blank unless handler behaviour proves
  them; the seven `BATTLEFIELD_MGR_*` disagreements in published community tables are settled by
  their handlers' own events.
- `3.3.5a-verb-events.tsv`: the 61 verb/event pairs fired inside the registered verb's own body. It
  is explicitly a floor: an event a verb fires through a helper is not listed.
- `3.3.5a-dbc.tsv`: all 241 DBC tables the client loads, one row per table loader (237 at startup,
  four on demand), with the column count and row size each loader checks and the number of
  functions that read it. Readers are a floor (a read through a computed pointer is not seen),
  complete for the twelve `gt` tables, and attributed to Lua bindings and opcode handlers that read
  directly. Ten tables have no reader: 12340 loads them and never reads them.

The ninth table, `3.3.5a-globals.tsv`, is the entire addon-visible global namespace: 46,312
names, `name<TAB>type<TAB>origin`. It unions 43,708 names captured in-world after loading all 23
shipped `Blizzard_*` addons (minus the capture addon's two globals) with 2,606 shipped-UI
definitions absent from that session as type `lod`. Its origin totals are 35 Lua 5.1, 2,079 engine
and 44,198 FrameXML names.

That table cannot come from the client's registry alone. The capture addon at
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

## The glue screens

The glue screens run their own Lua state: a different surface and a different event table from the
in-game state's, and GlueXML runs against those.
`crates/benilla-ui/src/script/tests/glue_surface.rs` holds the glue VM (`UiScript::new_glue`) to the
glue registrar tables of `3.3.5a-shapes.tsv`: every function it exposes is a glue-state one, and
every glue-state function it lacks is on the file's `NOT_YET` list, which only shrinks. The same file
holds the VM's event table to `3.3.5a-glue-events.tsv`.
