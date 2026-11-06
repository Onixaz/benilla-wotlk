# benilla's `lua-src` fork: what differs from upstream, and how to check

Upstream: [`mlua-rs/lua-src-rs`](https://github.com/mlua-rs/lua-src-rs), MIT, version `550.0.0`,
the version mlua 0.11 resolves to. Wired in through `[patch.crates-io]` in the workspace root.

## The Lua sources are upstream's, but for the owner tag

`lua-5.1.5/` is upstream's tree but for the hunk below. The reference client, 3.3.5a, embeds stock Lua 5.1
(`$Lua: Lua 5.1.1` at `0xa470a0` in `Wow.exe`), and its own strings show the grammar and messages
are 5.1's:

| fact | in 3.3.5a's `.rdata` |
|---|---|
| `#` and `%` are operators | metamethod names `__len` `0xa48184`, `__mod` `0xa4819c` |
| `...` is an expression | `cannot use '...' outside a vararg function` `0xa488d8`; opcode name `VARARG` |
| no table-iterating generic-for | the opcode names at `0xa48318` run `TFORLOOP`, `FORPREP`: no 5.0 `TFORPREP` |
| `[[...]]` does not nest (`LUA_COMPAT_LSTR` 1) | `nesting of [[...]] is deprecated` `0xa48678` |
| errors quote `'x'` (`LUA_QL`) | `%s near '%s'` `0xa48630`, `attempt to %s %s '%s' (a %s value)` `0xa474c4` |

3.3.5a is 5.1.1 and this tree is 5.1.5; the difference is upstream's own bug fixes.

## Why a fork still exists

`src/lib.rs` differs from upstream by having Lua 5.2/5.3/5.4/5.5 stripped from the `Version` enum
(with their source trees deleted): benilla builds 5.1 and only 5.1, and a fork that still offered
the other four would answer a request for one with a missing directory at build time instead of a
compile error. A change the reference's VM makes to stock 5.1 lands here, as a listed hunk.

## The owner tag

`GetAddOnMemoryUsage` answers per addon because the reference's VM carries a taint pointer in every
object's header and walks the heap for it (`0x85b4e0`). benilla keeps no taint, so the hunk carries
only the owner:

- `lobject.h`: a `lu_byte owner` in `CommonHeader`, which fits the header's padding, so no object
  grows. 255 owners; every addon past the 255th shares the last.
- `lua.h`, `lgc.h`, `lgc.c`: the running owner, `lua_setgcowner`, and `lua_gcupdatesizes`, the walk
  of `rootgc` and the string table.
- `lgc.c`, `lstring.c`, `lfunc.c`, `lstate.c`: a new object takes the running owner.
- `lvm.c`: entering a closure with an owner makes it the running one, as the reference's closure
  taint does (`0x857ca0`).
- `ldo.c`: a call from outside Lua starts from the owner the host set.

## The command that proves it

```sh
# 550.0.0 is the pinned version; adjust the path if cargo's registry hash differs.
diff -r --strip-trailing-cr third_party/lua-src/lua-5.1.5 \
  ~/.cargo/registry/src/*/lua-src-550.0.0/lua-5.1.5
```

That must print the owner tag's hunks and nothing else.
