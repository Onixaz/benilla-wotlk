# benilla

A from-scratch World of Warcraft client in Rust and Bevy.

Upstream benilla targets World of Warcraft 1.12.1. This fork targets World of Warcraft 3.3.5a
(build 12340) alone, under the same core rule:

**the reference client is the specification.**

The reference client is 3.3.5a. Where code or a comment still cites a 1.12.1 fact for behaviour
3.3.5a must define, establish it against the 3.3.5a client before relying on it.

Read, in this order:

1. `docs/METHOD.md` — the rules and working method. Binding for every session.
2. `docs/MAP.md` — what is built right now, generated from the code. Orient here before touching anything.
3. `docs/CONTRIBUTING.md` — what gets in and how a change is judged.

The rest of the root is code and data: `crates/` is the workspace, `reference/` contains the reference surfaces checked by tests, `scripts/` contains gates and development instruments, and `third_party/` contains vendored code.

When the user says **check upstream merges** or **sync upstream tracker**, follow
`docs/UPSTREAM_TRACKER.md`.

@docs/METHOD.md
