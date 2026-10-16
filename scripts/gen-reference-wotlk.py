#!/usr/bin/env python3
"""Generate verified reference surfaces from WoW.exe 3.3.5a build 12340.

    scripts/gen-reference-wotlk.py --exe PATH [--out-dir DIR]

The executable is an input, never an output and never copied into the repository. The generator
accepts only the verified enUS build-12340 binary whose SHA-256 is recorded below. Checked-in
outputs contain structural names and addresses, not executable bytes.

This script intentionally starts with surfaces whose extraction is mechanically complete. Add a
surface only when its build-12340 mechanism and completeness checks are known; never fill a WotLK
gap from the Classic tables.
"""

import argparse
import bisect
import collections
import hashlib
import os
import re
import shutil
import struct
import subprocess
import sys


REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REFERENCE = os.path.join(REPO, "reference")

BUILD = 12340
VERSION = "3.3.5a"
WOW_SHA256 = "aa63a5750d60ef16746c686b3d5e26876d98953eab08b1c026cd0faf78e88cb8"

# `0x608880` writes the world event-name pointer array and returns at `0x60a111`. The caller at
# `0x52ab1b` passes `(0xc24eb0, 0x2d2)` to the FrameScript event registry at `0x81b5f0`.
EVENT_INIT = (0x00608880, 0x0060A112)
EVENT_NAMES = 0x00C24EB0
EVENT_COUNT = 0x2D2

# GlueXML swaps in a separate 41-slot registry at startup (`0x4da749`). Its producers live in a
# contiguous Glue subsystem range; their numeric ids must never be resolved through the world
# catalog above (id 23 is `UPDATE_SELECTED_RACE` there, not world `UNIT_HAPPINESS`).
GLUE_EVENT_INIT = (0x004DBC70, 0x004DBE0B)
GLUE_EVENT_NAMES = 0x00B6AFF8
GLUE_EVENT_COUNT = 0x29
GLUE_SITE_RANGE = (0x004D7000, 0x004E6000)

# Both public entry points forward `(eventId, arg_format, varargs)` to `0x81ac90`. The second
# wrapper first checks whether the world event system is live. There are no direct callers of the
# internal formatter other than these wrappers, and no tail jumps to either wrapper in `.text`.
EVENT_DISPATCHERS = {
    0x0081B530: "SignalEvent",
    0x005326D0: "SignalEventGuarded",
}

# The world-session dispatcher at 0x63200b rejects opcode values >= 0x51f, then indexes two
# parallel four-byte arrays on the session object: a handler pointer at +0x53c and handler context
# at +0x19b8. `0x631fa0(opcode, handler, context)` fills those arrays; most subsystems call it
# through the process-global wrapper at 0x6b0b80. The binary contains names for only a handful of
# opcodes, so the generated surface keeps names blank unless handler behaviour proves one.
OPCODE_COUNT = 0x51F
OPCODE_REGISTER = 0x00631FA0
OPCODE_REGISTER_WRAPPER = 0x006B0B80
OPCODE_REGISTER_WRAPPER_CALL = 0x006B0B9D

# These seven disputed names are proved without a server table: each opcode has its own registered
# handler, and that handler directly fires the matching build-12340 FrameScript event at the site
# below. They also serve as end-to-end anchors for registration and handler-event attribution.
OPCODE_EVENT_NAME_ANCHORS = {
    0x4DE: (0x0054B610, 0x0054B663, "BATTLEFIELD_MGR_ENTRY_INVITE"),
    0x4E0: (0x005498C0, 0x005499AE, "BATTLEFIELD_MGR_ENTERED"),
    0x4E1: (0x0054B7A0, 0x0054B7D8, "BATTLEFIELD_MGR_QUEUE_INVITE"),
    0x4E4: (0x0054B680, 0x0054B73A, "BATTLEFIELD_MGR_QUEUE_REQUEST_RESPONSE"),
    0x4E5: (0x0054B750, 0x0054B787, "BATTLEFIELD_MGR_EJECT_PENDING"),
    0x4E6: (0x005499C0, 0x00549AA1, "BATTLEFIELD_MGR_EJECTED"),
    0x4E8: (0x0054B7F0, 0x0054B822, "BATTLEFIELD_MGR_STATE_CHANGE"),
}

# The unit-field watcher registers fields 0..0x8d with callback 0x60c240. That callback derives
# the event id from the changed field and enters 0x60bf10, which expands a unit GUID to the matching
# Lua unit tokens and fires `(eventId, "%s", token)`. Other callers use the same helper with an
# explicit event id for UNIT_* changes that are not driven by the field window.
UNIT_FIELD_EVENT_COUNT = 0x8E
UNIT_TOKEN_FANOUT = 0x0060BF10
UNIT_TOKEN_FANOUT_IDS = frozenset(
    (0, 2, 17, 18, 26, 48, 49, 144, 145, 146, 147, 148, 149, 336, 399, 460, 525, 526, 589)
)

# `RegisterFunction(name, fn)` and the widget method-table registrar. The former is reached from
# one counted loop per engine table; the latter takes `(lua, table, count)` directly.
REGISTER_FUNCTION = 0x00817F90
REGISTER_METHODS = 0x008167E0

# Build-12340's Lua 5.1 API is linked into `.text`. These entry points are calibrated from the
# shipped base-library bindings, not borrowed from the Classic client: `type`, `rawequal`,
# `rawget`, `rawset` and `assert` jointly identify the check/read/push operations below. The
# generator validates those call patterns before using the map.
LUA_GETTOP = 0x0084DBD0
LUA_SETTOP = 0x0084DBF0
LUA_PUSHVALUE = 0x0084DE50
LUA_TYPE = 0x0084DEB0
LUA_TYPENAME = 0x0084DED0
LUA_ISCFUNCTION = 0x0084DEF0
LUA_ISNUMBER = 0x0084DF20
LUA_ISSTRING = 0x0084DF60
LUA_RAWEQUAL = 0x0084DF90
LUA_EQUAL = 0x0084DFE0
LUA_TONUMBER = 0x0084E030
LUA_TOINTEGER = 0x0084E070
LUA_TOBOOLEAN = 0x0084E0B0
LUA_TOLSTRING = 0x0084E0E0
LUA_OBJLEN = 0x0084E150
LUA_TOCFUNCTION = 0x0084E1C0
LUA_TOUSERDATA = 0x0084E1F0
LUA_TOTHREAD_OR_POINTER = 0x0084E210
LUA_PUSHNIL = 0x0084E280
LUA_PUSHNUMBER = 0x0084E2A0
LUA_PUSHINTEGER = 0x0084E2D0
LUA_PUSHLSTRING = 0x0084E300
LUA_PUSHSTRING = 0x0084E350
LUA_PUSHVFSTRING = 0x0084E3A0
LUA_PUSHFSTRING = 0x0084E3D0
LUA_PUSHCCLOSURE = 0x0084E400
LUA_PUSHBOOLEAN = 0x0084E4D0
LUA_PUSHLIGHTUSERDATA = 0x0084E500
LUA_PUSHTHREAD = 0x0084E530
LUA_GETTABLE = 0x0084E560
LUA_GETFIELD = 0x0084E590
LUA_RAWGET = 0x0084E600
LUA_RAWGETI = 0x0084E670
LUA_CREATETABLE = 0x0084E6E0
LUA_GETMETATABLE = 0x0084E730
LUA_GETFENV = 0x0084E7A0
LUA_SETTABLE = 0x0084E8D0
LUA_SETFIELD = 0x0084E900
LUA_RAWSET = 0x0084E970
LUA_RAWSETI = 0x0084EA00
LUA_SETMETATABLE = 0x0084EA90
LUA_SETFENV = 0x0084EB40
LUAL_CHECKTYPE = 0x0084F960
LUAL_CHECKANY = 0x0084F9C0
LUAL_CHECKLSTRING = 0x0084F9F0
LUAL_OPTLSTRING = 0x0084FA50
LUAL_CHECKNUMBER = 0x0084FAB0
LUAL_OPTNUMBER = 0x0084FB30
LUAL_CHECKINTEGER = 0x0084FB60
LUAL_OPTINTEGER = 0x0084FBD0

# Positions are cdecl argument positions including lua_State* at position zero. Only positive Lua
# stack indices contribute to argc; negative top-relative and pseudo-indices do not.
LUA_INDEX_ARGUMENTS = {
    LUA_PUSHVALUE: (1,),
    LUA_TYPE: (1,),
    LUA_ISCFUNCTION: (1,),
    LUA_ISNUMBER: (1,),
    LUA_ISSTRING: (1,),
    LUA_RAWEQUAL: (1, 2),
    LUA_EQUAL: (1, 2),
    LUA_TONUMBER: (1,),
    LUA_TOINTEGER: (1,),
    LUA_TOBOOLEAN: (1,),
    LUA_TOLSTRING: (1,),
    LUA_OBJLEN: (1,),
    LUA_TOCFUNCTION: (1,),
    LUA_TOUSERDATA: (1,),
    LUA_TOTHREAD_OR_POINTER: (1,),
    LUA_GETTABLE: (1,),
    LUA_GETFIELD: (1,),
    LUA_RAWGET: (1,),
    LUA_RAWGETI: (1,),
    LUA_GETMETATABLE: (1,),
    LUA_GETFENV: (1,),
    LUA_SETTABLE: (1,),
    LUA_SETFIELD: (1,),
    LUA_RAWSET: (1,),
    LUA_RAWSETI: (1,),
    LUA_SETMETATABLE: (1,),
    LUA_SETFENV: (1,),
    LUAL_CHECKTYPE: (1,),
    LUAL_CHECKANY: (1,),
    LUAL_CHECKLSTRING: (1,),
    LUAL_OPTLSTRING: (1,),
    LUAL_CHECKNUMBER: (1,),
    LUAL_OPTNUMBER: (1,),
    LUAL_CHECKINTEGER: (1,),
    LUAL_OPTINTEGER: (1,),
}

# Stack effects used only for return-kind evidence. `string?` is intentional: lua_pushstring
# pushes nil for a null pointer. Less specific but still sound operations are recorded as `any`.
LUA_PUSH_KINDS = {
    LUA_PUSHVALUE: "any",
    LUA_PUSHNIL: "nil",
    LUA_PUSHNUMBER: "number",
    LUA_PUSHINTEGER: "number",
    LUA_PUSHLSTRING: "string",
    LUA_PUSHSTRING: "string?",
    LUA_PUSHVFSTRING: "string",
    LUA_PUSHFSTRING: "string",
    LUA_PUSHCCLOSURE: "any",
    LUA_PUSHBOOLEAN: "boolean",
    LUA_PUSHLIGHTUSERDATA: "any",
    LUA_PUSHTHREAD: "any",
    LUA_GETFIELD: "any",
    LUA_RAWGETI: "any",
    LUA_CREATETABLE: "table",
    LUA_GETFENV: "any",
}
LUA_REPLACE_TOP_KINDS = {LUA_GETTABLE: "any", LUA_RAWGET: "any"}
LUA_STACK_RESETTERS = {
    LUA_SETTOP,
    LUA_TOLSTRING,
    LUA_GETMETATABLE,
    LUA_SETTABLE,
    LUA_SETFIELD,
    LUA_RAWSET,
    LUA_RAWSETI,
    LUA_SETMETATABLE,
    LUA_SETFENV,
    LUAL_CHECKLSTRING,
    LUAL_OPTLSTRING,
}
LUA_STACK_PRESERVING = (
    set(LUA_INDEX_ARGUMENTS)
    | {LUA_GETTOP, LUA_TYPENAME}
) - set(LUA_PUSH_KINDS) - set(LUA_REPLACE_TOP_KINDS) - LUA_STACK_RESETTERS
LUA_KNOWN_TARGETS = (
    set(LUA_INDEX_ARGUMENTS)
    | set(LUA_PUSH_KINDS)
    | set(LUA_REPLACE_TOP_KINDS)
    | LUA_STACK_RESETTERS
    | LUA_STACK_PRESERVING
    | {LUA_GETTOP, LUA_TYPENAME}
)

# `0x5216f0` bounds-checks the id against 0x2da then indexes 20-byte rows at 0xac8af0.
# `0x513dd0` independently exposes the same table's key by id with the same bound.
MESSAGE_TABLE = 0x00AC8AF0
MESSAGE_COUNT = 0x2DA
MESSAGE_ROW_SIZE = 20

# `0x634e2a` sets ESI to `0x6348b0` (`mov eax,[ecx]; jmp [eax+4]`: a table object's load slot) and
# calls `0x6337d0`, a straight run of `mov ecx,<table object>; call esi` returning at `0x63478d`.
# Each table object is a 0x24-byte `.data` struct whose first dword is its vtable, statically
# initialised. Each loader names its file through a `mov eax,<path>; ret` thunk and checks the
# header against literal widths, passing them to one shared pair of format strings.
DBC_LOAD_ALL = (0x006337D0, 0x0063478D)
DBC_LOAD_SLOT = 0x006348B0
DBC_OBJECT_SIZE = 0x24
DBC_COLUMNS_FORMAT = 0x009E24F4
DBC_ROW_SIZE_FORMAT = 0x009E24A4
DBC_TABLE_COUNT = 241
DBC_STARTUP_COUNT = 237
# `0x7f6990(slot, row, column)` reads a game table through the adapter at `[0xaf4f90 + 4*slot]`;
# `0x7f6a80` binds the twelve adapters (`cmp esi,0x30`) by name, and each adapter's `+0xc` is the
# table object it reads.
GAME_TABLE_ADAPTERS = 0x00AF4F90
GAME_TABLE_COUNT = 12
GAME_TABLE_LOOKUP = 0x007F6990


class PeImage:
    """The small PE32 view this generator needs: sections, VA mapping and C strings."""

    def __init__(self, path):
        self.path = path
        self.data = open(path, "rb").read()
        if self.data[:2] != b"MZ":
            sys.exit(f"{path} is not a PE executable")
        pe = struct.unpack_from("<I", self.data, 0x3C)[0]
        if self.data[pe : pe + 4] != b"PE\0\0":
            sys.exit(f"{path} has no PE signature")
        machine, count, _time, _sym, _n_sym, optional_size, _flags = struct.unpack_from(
            "<HHIIIHH", self.data, pe + 4
        )
        if machine != 0x14C:
            sys.exit(f"{path} is not an i386 executable (machine {machine:#x})")
        optional = pe + 24
        if struct.unpack_from("<H", self.data, optional)[0] != 0x10B:
            sys.exit(f"{path} is not PE32")
        self.image_base = struct.unpack_from("<I", self.data, optional + 28)[0]
        self.sections = []
        section_table = optional + optional_size
        for i in range(count):
            off = section_table + 40 * i
            name = self.data[off : off + 8].split(b"\0", 1)[0].decode("ascii", "replace")
            virtual_size, rva, raw_size, raw = struct.unpack_from("<IIII", self.data, off + 8)
            self.sections.append(
                {
                    "name": name,
                    "va": self.image_base + rva,
                    "virtual_size": virtual_size,
                    "raw": raw,
                    "raw_size": raw_size,
                }
            )

    def section(self, name):
        matches = [s for s in self.sections if s["name"] == name]
        if len(matches) != 1:
            sys.exit(f"{self.path}: expected one {name} section, found {len(matches)}")
        return matches[0]

    def offset(self, va, size=1):
        for section in self.sections:
            delta = va - section["va"]
            if 0 <= delta and delta + size <= section["raw_size"]:
                return section["raw"] + delta
        raise ValueError(f"VA {va:#x} ({size} bytes) is not backed by the executable")

    def bytes_at(self, va, size):
        off = self.offset(va, size)
        return self.data[off : off + size]

    def cstring(self, va):
        off = self.offset(va)
        end = self.data.find(b"\0", off)
        if end < 0:
            raise ValueError(f"unterminated string at {va:#x}")
        return self.data[off:end].decode("ascii")


def checked_image(path):
    digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
    if digest != WOW_SHA256:
        sys.exit(
            f"{path} is not the verified {VERSION} build {BUILD} reference executable\n"
            f"  expected sha256 {WOW_SHA256}\n"
            f"       got sha256 {digest}"
        )
    image = PeImage(path)
    expected = {
        ".text": (0x00401000, 0x005DD400),
        ".rdata": (0x009DF000, 0x000D6800),
        ".data": (0x00AB6000, 0x00078E00),
    }
    for name, (va, raw_size) in expected.items():
        section = image.section(name)
        got = (section["va"], section["raw_size"])
        if got != (va, raw_size):
            sys.exit(f"{path}: {name} is {got}, expected {(va, raw_size)}")
    return image


def initializer_stores(image, span):
    """Decode the constant stores made by one straight-line name-table initializer."""
    start, end = span
    code = image.bytes_at(start, end - start)
    registers = [None] * 8  # eax, ecx, edx, ebx, esp, ebp, esi, edi
    stores = {}
    pos = 0
    while pos < len(code):
        opcode = code[pos]
        if 0xB8 <= opcode <= 0xBF:  # mov reg32, imm32
            if pos + 5 > len(code):
                break
            registers[opcode - 0xB8] = struct.unpack_from("<I", code, pos + 1)[0]
            pos += 5
        elif opcode == 0xA3:  # mov [imm32], eax
            dest = struct.unpack_from("<I", code, pos + 1)[0]
            stores[dest] = registers[0]
            pos += 5
        elif opcode == 0x89 and pos + 6 <= len(code) and code[pos + 1] & 0xC7 == 0x05:
            # mov [imm32], reg32
            reg = (code[pos + 1] >> 3) & 7
            dest = struct.unpack_from("<I", code, pos + 2)[0]
            stores[dest] = registers[reg]
            pos += 6
        elif (
            opcode == 0xC7
            and pos + 10 <= len(code)
            and code[pos + 1] == 0x05
        ):  # mov dword ptr [imm32], imm32
            dest, value = struct.unpack_from("<II", code, pos + 2)
            stores[dest] = value
            pos += 10
        elif opcode == 0xC3 and pos == len(code) - 1:
            pos += 1
        else:
            sys.exit(
                f"unrecognised instruction in the event initializer at {start + pos:#x}: "
                f"{code[pos:pos + 10].hex(' ')}"
            )

    return stores


def catalog_from_initializer(image, span, names_va, count):
    stores = initializer_stores(image, span)
    rows = []
    for event_id in range(count):
        slot = names_va + 4 * event_id
        pointer = stores.get(slot)
        if pointer is None:
            continue
        try:
            name = image.cstring(pointer)
        except (UnicodeDecodeError, ValueError) as error:
            sys.exit(f"event {event_id}'s pointer {pointer:#x} is not an ASCII string: {error}")
        # Slot 237 is explicitly assigned `0x9e14ff`, the empty byte between the saved-account
        # CVar text and `perf`; like an uninitialised slot it names no event.
        if not name:
            continue
        if any(c not in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_" for c in name):
            sys.exit(f"event {event_id}'s pointer {pointer:#x} names {name!r}, not an event")
        rows.append((event_id, name))

    return rows


def event_catalog(image):
    """Decode every named slot in the build-12340 world and glue event registries."""
    rows = catalog_from_initializer(image, EVENT_INIT, EVENT_NAMES, EVENT_COUNT)
    if len(rows) < 600:
        sys.exit(f"decoded only {len(rows)} of {EVENT_COUNT} world event slots")
    if rows[0] != (0, "UNIT_PET"):
        sys.exit(f"event catalog starts with {rows[0]!r}, expected UNIT_PET")
    by_id = dict(rows)
    expected = {
        18: "UNIT_HEALTH",
        187: "UI_ERROR_MESSAGE",
        188: "UI_INFO_MESSAGE",
        EVENT_COUNT - 1: "CHAT_MSG_BN_INLINE_TOAST_CONVERSATION",
    }
    for event_id, name in expected.items():
        if by_id.get(event_id) != name:
            sys.exit(
                f"event {event_id} is {by_id.get(event_id)!r}, expected {name!r}; "
                "the build-12340 layout changed"
            )
    glue = catalog_from_initializer(
        image, GLUE_EVENT_INIT, GLUE_EVENT_NAMES, GLUE_EVENT_COUNT
    )
    if len(glue) != GLUE_EVENT_COUNT:
        sys.exit(f"decoded only {len(glue)} of {GLUE_EVENT_COUNT} glue event slots")
    if glue[0] != (0, "SET_GLUE_SCREEN") or glue[-1] != (
        GLUE_EVENT_COUNT - 1,
        "CLIENT_CONVERTED",
    ):
        sys.exit(f"glue event catalog endpoints changed: {glue[0]!r}, {glue[-1]!r}")
    return rows, glue


def write_event_catalog(path, rows):
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# FrameScript C->Lua world-event catalog of the {VERSION} client "
            f"(WoW.exe build {BUILD}): eventId -> name.\n"
            f"# Source: the {EVENT_COUNT:#x}-slot event-name pointer array at {EVENT_NAMES:#x}, "
            f"filled by {EVENT_INIT[0]:#x}..{EVENT_INIT[1] - 1:#x}.\n"
            f"# The caller at 0x52ab1b passes ({EVENT_NAMES:#x}, {EVENT_COUNT:#x}) to the "
            "FrameScript event registry at 0x81b5f0.\n"
            "eventId\tname\n"
        )
        for event_id, name in rows:
            out.write(f"{event_id}\t{name}\n")


def write_glue_event_catalog(path, rows):
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# FrameScript C->Lua glue-event catalog of the {VERSION} client "
            f"(WoW.exe build {BUILD}): eventId -> name.\n"
            f"# Source: the {GLUE_EVENT_COUNT:#x}-slot event-name pointer array at "
            f"{GLUE_EVENT_NAMES:#x}, filled by {GLUE_EVENT_INIT[0]:#x}..{GLUE_EVENT_INIT[1] - 1:#x}.\n"
            "# The glue manager's Resume (0x4da5f0) hands it to the FrameScript event registry "
            "at 0x81b5f0 in place of the world table, so a glue event's id names a different "
            "event in-game.\n"
            "eventId\tname\n"
        )
        for event_id, name in rows:
            out.write(f"{event_id}\t{name}\n")


def disassembly(image, objdump):
    """Return objdump's one-instruction-per-line view of `.text`.

    This surface needs instruction boundaries, not a byte-pattern search: a compiler may push the
    event arguments, initialise locals, and only then make the call. GNU objdump is deliberately an
    explicit regeneration dependency instead of embedding an incomplete x86 decoder here.
    """
    text = image.section(".text")
    command = [
        objdump,
        "-d",
        "--insn-width=16",
        f"--start-address={text['va']:#x}",
        f"--stop-address={text['va'] + text['raw_size']:#x}",
        image.path,
    ]
    try:
        result = subprocess.run(command, check=True, capture_output=True, text=True)
    except (OSError, subprocess.CalledProcessError) as error:
        detail = getattr(error, "stderr", "")
        sys.exit(f"could not disassemble {image.path} with {objdump}: {error}\n{detail}")

    line_re = re.compile(
        r"^\s*([0-9a-fA-F]+):\s+((?:[0-9a-fA-F]{2}\s+)+)\s*"
        r"([a-zA-Z][a-zA-Z0-9.]*)\s*(.*?)\s*$"
    )
    instructions = []
    for line in result.stdout.splitlines():
        match = line_re.match(line)
        if not match:
            continue
        instructions.append(
            {
                "va": int(match.group(1), 16),
                "bytes": bytes.fromhex(match.group(2)),
                "mnemonic": match.group(3),
                "operands": match.group(4),
            }
        )
    if len(instructions) < 500_000:
        sys.exit(f"objdump yielded only {len(instructions)} .text instructions")
    return instructions


def direct_target(instruction):
    """Resolve an x86 rel32 call/jump from its bytes, independent of objdump's spelling."""
    raw = instruction["bytes"]
    if len(raw) != 5 or raw[0] not in (0xE8, 0xE9):
        return None
    displacement = struct.unpack_from("<i", raw, 1)[0]
    return instruction["va"] + 5 + displacement


def push_value(instruction):
    """Return an immediate push's value; computed/register/memory pushes remain unknown."""
    raw = instruction["bytes"]
    if len(raw) == 2 and raw[0] == 0x6A:
        return struct.unpack_from("<b", raw, 1)[0] & 0xFFFFFFFF
    if len(raw) == 5 and raw[0] == 0x68:
        return struct.unpack_from("<I", raw, 1)[0]
    return None


def pushed_arguments(instructions, call_index):
    """Conservatively recover the final two cdecl pushes reaching a dispatcher call.

    Straight-line local initialisation between a push and call is harmless and common in this
    binary. Crossing another call or control-flow boundary is not: doing that would silently pick
    a push from another path. Such arguments are recorded as computed instead.
    """
    pushes = []
    for instruction in reversed(instructions[max(0, call_index - 48) : call_index]):
        mnemonic = instruction["mnemonic"]
        if mnemonic.startswith("push"):
            pushes.append(push_value(instruction))
            if len(pushes) == 2:
                break
            continue
        if (
            mnemonic.startswith("call")
            or mnemonic.startswith("ret")
            or mnemonic in ("int3", "leave", "jmp", "ljmp")
            or (mnemonic.startswith("j") and mnemonic not in ("jmp", "ljmp"))
        ):
            break
    while len(pushes) < 2:
        pushes.append(None)
    # cdecl pushes right to left: the nearest push is eventId, then the format pointer.
    return pushes[0], pushes[1]


def format_string(image, pointer):
    if pointer is None:
        return None
    if pointer == 0:
        return ""
    try:
        value = image.cstring(pointer)
    except (UnicodeDecodeError, ValueError):
        return None
    # The dispatcher consumes one vararg per directive. Literal strings at these callsites contain
    # only the small format alphabet below; rejecting prose also catches a bad stack recovery.
    if not re.fullmatch(r"(?:%[sduf])*(?:%%(?:%[sduf])*)*", value):
        return None
    return value


def event_firesites(image, instructions, catalog):
    by_id = dict(catalog)
    rows = []
    target_counts = {target: 0 for target in EVENT_DISPATCHERS}
    tail_jumps = 0
    for index, instruction in enumerate(instructions):
        target = direct_target(instruction)
        if target not in EVENT_DISPATCHERS:
            continue
        if instruction["bytes"][0] == 0xE9:
            tail_jumps += 1
            continue
        target_counts[target] += 1
        event_id, format_pointer = pushed_arguments(instructions, index)
        if event_id is not None and event_id >= EVENT_COUNT:
            # A recovered constant outside the registry is evidence that control flow separated the
            # true argument pushes; retain the site but do not publish a false event id.
            event_id = None
        arg_format = format_string(image, format_pointer)
        glue_site = GLUE_SITE_RANGE[0] <= instruction["va"] < GLUE_SITE_RANGE[1]
        rows.append(
            (
                instruction["va"],
                EVENT_DISPATCHERS[target],
                event_id,
                by_id.get(event_id, "") if event_id is not None and not glue_site else "",
                arg_format,
            )
        )

    expected_counts = {0x0081B530: 1036, 0x005326D0: 46}
    if target_counts != expected_counts or tail_jumps:
        sys.exit(
            f"dispatcher xrefs changed: calls={target_counts}, tail jumps={tail_jumps}; "
            f"expected {expected_counts} and zero"
        )
    if len(rows) != 1082:
        sys.exit(f"decoded {len(rows)} event callsites, expected 1082")
    known_ids = sum(row[2] is not None for row in rows)
    known_formats = sum(row[4] is not None for row in rows)
    if known_ids < 800 or known_formats < 650:
        sys.exit(
            f"conservative recovery fell below its build-12340 floor: "
            f"{known_ids} ids, {known_formats} formats"
        )
    return rows


def write_event_firesites(path, rows):
    known_ids = sum(row[2] is not None for row in rows)
    known_formats = sum(row[4] is not None for row in rows)
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# C->Lua event fire sites of the {VERSION} client (WoW.exe build {BUILD}): every "
            "direct call in `.text` to\n"
            "# FrameScript_SignalEvent (0x81b530) and its world-live guarded wrapper "
            "(0x5326d0). Both forward\n"
            "# `(eventId, arg_format, varargs)` to the formatter at 0x81ac90. There are no tail "
            "jumps to either.\n"
            "#\n"
            "# `dyn`/`?` is deliberately conservative: objdump established instruction "
            "boundaries, but a value was\n"
            "# computed or its defining push lay across a control-flow boundary. It is not "
            "filled from another client.\n"
            "# A blank arg_format is a literal null pointer and therefore zero Lua arguments; "
            "`?` is unresolved.\n"
            f"# Sites in {GLUE_SITE_RANGE[0]:#x}..{GLUE_SITE_RANGE[1] - 1:#x} use the separate "
            f"{GLUE_EVENT_COUNT}-slot GlueXML registry at\n"
            f"# {GLUE_EVENT_NAMES:#x}; their names stay blank rather than being falsely resolved "
            "through the world table.\n"
            f"# {len(rows)} callsites; {known_ids} literal event ids; {known_formats} literal "
            "format pointers (including null).\n"
            "fire_site_va\tdispatcher\teventId\tname\targ_format\n"
        )
        for va, dispatcher, event_id, name, arg_format in rows:
            out.write(
                f"0x{va:x}\t{dispatcher}\t"
                f"{event_id if event_id is not None else 'dyn'}\t{name}\t"
                f"{arg_format if arg_format is not None else '?'}\n"
            )


def computed_event_families(instructions):
    """Recover the event ids passed explicitly to the verified unit-token fan-out helper."""
    call_count = 0
    event_ids = set()
    for index, instruction in enumerate(instructions):
        if direct_target(instruction) != UNIT_TOKEN_FANOUT or instruction["bytes"][0] != 0xE8:
            continue
        call_count += 1
        # cdecl pushes `(unit, eventId)` right to left: nearest is unit, second-nearest is eventId.
        event_id = instruction_pushes(instructions, index, 2)[1]
        if event_id is not None and event_id < EVENT_COUNT:
            event_ids.add(event_id)

    if call_count != 55:
        sys.exit(f"unit-token fan-out xrefs changed: {call_count}, expected 55")
    if event_ids != UNIT_TOKEN_FANOUT_IDS:
        sys.exit(
            f"unit-token fan-out ids changed: {sorted(event_ids)}, "
            f"expected {sorted(UNIT_TOKEN_FANOUT_IDS)}"
        )
    return event_ids


def write_events(path, catalog, firesites, token_fanout_ids):
    """Aggregate literal sites plus the two verified computed-id unit producer families."""
    events = {}
    for event_id, name in catalog:
        row = events.setdefault(
            name,
            {"ids": set(), "producers": set(), "formats": set(), "unresolved": 0},
        )
        row["ids"].add(event_id)
    for _va, dispatcher, event_id, name, arg_format in firesites:
        if event_id is None or not name:
            continue
        row = events[name]
        row["producers"].add(
            "signal-guarded" if dispatcher == "SignalEventGuarded" else "signal"
        )
        if arg_format is None:
            row["unresolved"] += 1
        else:
            row["formats"].add(arg_format or "()")

    for event_id, name in catalog:
        row = events[name]
        if event_id < UNIT_FIELD_EVENT_COUNT:
            row["producers"].add("unit-field-bridge")
            row["formats"].add("%s")
        if event_id in token_fanout_ids:
            row["producers"].add("token-fanout")
            row["formats"].add("%s")

    produced = sum(bool(row["producers"]) for row in events.values())
    unresolved = sum(row["unresolved"] for row in events.values())
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# The {VERSION} client's FrameScript events and argument formats from attributed "
            "producer families.\n"
            "# Generated with the catalog and callsite passes in scripts/gen-reference-wotlk.py.\n"
            "#\n"
            "# This remains advisory: the unit-field bridge and unit-token fan-out are mapped, "
            "but other\n"
            "# computed-id producer paths may remain. `advisory` means the listed shapes are real "
            "but may not be\n"
            "# exhaustive; `none` means neither a literal-id site nor a mapped computed family "
            "proved a producer.\n"
            "# arg_formats uses `|` for distinct shapes and `()` for zero Lua values. `?` means "
            "a literal-id\n"
            "# site crossed a control-flow boundary before its format pointer could be proven.\n"
            f"# {len(events)} named events; {produced} with an attributed producer; {unresolved} "
            "unresolved formats at literal-id sites.\n"
            "name\tids\tproducers\targ_formats\tconf\tnote\n"
        )
        for name in sorted(events):
            row = events[name]
            formats = set(row["formats"])
            if row["unresolved"]:
                formats.add("?")
            confidence = "advisory" if row["producers"] else "none"
            note = (
                "literal sites plus mapped unit families; other computed-id paths may remain"
                if row["producers"]
                else "no producer found at literal-id sites or mapped unit families"
            )
            out.write(
                "\t".join(
                    (
                        name,
                        "|".join(str(value) for value in sorted(row["ids"])),
                        "|".join(sorted(row["producers"])),
                        "|".join(sorted(formats)) or "?",
                        confidence,
                        note,
                    )
                )
                + "\n"
            )
    return len(events), produced


def instruction_pushes(instructions, call_index, count, lookback=32):
    """Immediate values of the nearest straight-line pushes, nearest first."""
    pushes = []
    for instruction in reversed(instructions[max(0, call_index - lookback) : call_index]):
        mnemonic = instruction["mnemonic"]
        if mnemonic.startswith("push"):
            pushes.append(push_value(instruction))
            if len(pushes) == count:
                return pushes
            continue
        if (
            mnemonic.startswith("call")
            or mnemonic.startswith("ret")
            or mnemonic in ("int3", "leave", "jmp", "ljmp")
            or (mnemonic.startswith("j") and mnemonic not in ("jmp", "ljmp"))
        ):
            break
    return pushes + [None] * (count - len(pushes))


def binding_pair(image, pair_va):
    name_pointer, function = struct.unpack("<II", image.bytes_at(pair_va, 8))
    try:
        name = image.cstring(name_pointer)
    except (UnicodeDecodeError, ValueError) as error:
        raise ValueError(f"binding pair {pair_va:#x} has no ASCII name: {error}") from error
    if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_:.]*", name):
        raise ValueError(f"binding pair {pair_va:#x} has invalid name {name!r}")
    text = image.section(".text")
    if not text["va"] <= function < text["va"] + text["raw_size"]:
        raise ValueError(f"binding {name} at {pair_va:#x} has non-code function {function:#x}")
    return name, function


def global_binding_table(image, instructions, call_index):
    """Recover one counted `RegisterFunction` loop's pair base and literal extent."""
    data = image.section(".data")
    indexed_addresses = []
    absolute_addresses = []
    for instruction in instructions[max(0, call_index - 10) : call_index]:
        for raw in re.findall(r"0x([0-9a-fA-F]+)\(%e[a-z]+\)", instruction["operands"]):
            address = int(raw, 16)
            if data["va"] <= address < data["va"] + data["raw_size"]:
                indexed_addresses.append(address)
        match = re.search(r"(?:^|,)0x([0-9a-fA-F]+),%e[a-z]+$", instruction["operands"])
        if match:
            address = int(match.group(1), 16)
            if data["va"] <= address < data["va"] + data["raw_size"]:
                absolute_addresses.append(address)
    candidates = sorted(set(indexed_addresses))
    bases = [value for value in candidates if value + 4 in candidates]
    if not bases:
        candidates = sorted(set(absolute_addresses))
        bases = [value for value in candidates if value + 4 in candidates]
        if len(bases) == 1:
            return bases[0], 1, "global"
    if len(bases) != 1:
        raise ValueError(
            f"RegisterFunction call {instructions[call_index]['va']:#x} has table operands "
            f"{[hex(value) for value in candidates]}"
        )
    table_va = bases[0]

    extent = None
    for instruction in instructions[call_index + 1 : call_index + 12]:
        match = re.fullmatch(r"\$0x([0-9a-fA-F]+),%e[a-z]+", instruction["operands"])
        if instruction["mnemonic"].startswith("cmp") and match:
            value = int(match.group(1), 16)
            if value and value % 8 == 0:
                extent = value
                break
    if extent is None:
        raise ValueError(
            f"RegisterFunction call {instructions[call_index]['va']:#x} has no literal extent"
        )
    return table_va, extent // 8, "global"


def binding_tables(image, instructions):
    """Discover every global, widget-method and bundled Lua-library registration table."""
    tables = []
    global_sites = method_sites = 0
    for index, instruction in enumerate(instructions):
        target = direct_target(instruction)
        if target == REGISTER_FUNCTION:
            global_sites += 1
            try:
                tables.append(global_binding_table(image, instructions, index))
            except ValueError as error:
                sys.exit(str(error))
        elif target == REGISTER_METHODS:
            method_sites += 1
            # Nearest is the Lua/object state, then table, then count.
            _state, table_va, count = instruction_pushes(instructions, index, 3)
            if table_va is None or count is None:
                sys.exit(
                    f"method registrar call {instruction['va']:#x} has no literal table/count"
                )
            tables.append((table_va, count, "method"))

    if (global_sites, method_sites) != (73, 39):
        sys.exit(
            f"binding registrar xrefs changed: {global_sites} global and {method_sites} method; "
            "expected 73 and 39"
        )

    # FrameScript's base-library tables and Lua 5.1's open-library tables are the null-terminated
    # name/function runs in `.rdata`. Requiring four rows excludes incidental pairs while the final
    # null pair proves the extent. The two base tables hold the global environment; the other five
    # are the bit/math/table/string/coroutine member tables.
    rdata = image.section(".rdata")
    raw = image.bytes_at(rdata["va"], rdata["raw_size"])
    offset = 0
    library_tables = []
    while offset + 8 <= len(raw):
        try:
            binding_pair(image, rdata["va"] + offset)
        except ValueError:
            offset += 4
            continue
        start = offset
        count = 0
        while offset + 8 <= len(raw):
            try:
                binding_pair(image, rdata["va"] + offset)
            except ValueError:
                break
            count += 1
            offset += 8
        terminator = (
            struct.unpack_from("<II", raw, offset) == (0, 0) if offset + 8 <= len(raw) else False
        )
        if count >= 4 and terminator:
            table_va = rdata["va"] + start
            table_kind = "baselib" if table_va in (0x00A443A8, 0x00A47C80) else "stdlib"
            library_tables.append((table_va, count, table_kind))
        else:
            offset = start + 4
    if len(library_tables) != 7:
        sys.exit(f"found {len(library_tables)} Lua library tables, expected 7")
    tables.extend(library_tables)

    # Alternate world/glue startup paths call some registration loops more than once. A table is
    # a surface once regardless of how many startup routes reach it.
    tables = sorted(set(tables), key=lambda row: (row[0], row[2]))
    rows = []
    for table_va, count, table_kind in tables:
        for index in range(count):
            pair_va = table_va + 8 * index
            try:
                name, function = binding_pair(image, pair_va)
            except ValueError as error:
                sys.exit(
                    f"{table_kind} table {table_va:#x} says it has {count} rows, but {error}"
                )
            rows.append((name, function, pair_va, table_va, table_kind))
    return tables, rows


def function_starts(instructions, bindings):
    """Conservative function entries: registrations, direct calls and post-padding code."""
    starts = {row[1] for row in bindings}
    starts.add(instructions[0]["va"])
    after_padding = False
    ended = False
    for instruction in instructions:
        mnemonic = instruction["mnemonic"]
        if mnemonic == "int3" or (mnemonic.startswith("nop") and (ended or after_padding)):
            after_padding = True
            continue
        if after_padding:
            starts.add(instruction["va"])
            after_padding = False
        target = direct_target(instruction)
        if target is not None and instruction["bytes"][0] == 0xE8:
            starts.add(target)
        ended = mnemonic.startswith("ret")
    return sorted(starts)


def branch_target(instruction):
    match = re.fullmatch(r"0x([0-9a-fA-F]+)", instruction["operands"])
    return int(match.group(1), 16) if match else None


def merge_values(left, right):
    """Join EAX constant sets; None is an unknown value, not an unreachable state."""
    if left is None or right is None:
        return None
    merged = left | right
    return merged if len(merged) <= 32 else None


def map_values(values, operation):
    if values is None:
        return None
    return frozenset(operation(value) & 0xFFFFFFFF for value in values)


def eax_after(instruction, values, callee_returns=None):
    """Transfer the EAX constants across one instruction; unknown writes stay unknown."""
    raw = instruction["bytes"]
    mnemonic = instruction["mnemonic"].lower()
    operands = instruction["operands"].replace(" ", "")

    if len(raw) == 5 and raw[0] == 0xB8:  # mov imm32, eax
        return frozenset((struct.unpack_from("<I", raw, 1)[0],))
    if raw in (b"\x31\xc0", b"\x33\xc0", b"\x29\xc0", b"\x2b\xc0"):
        return frozenset((0,))  # xor/sub eax,eax
    if mnemonic.startswith("call"):
        target = direct_target(instruction)
        if callee_returns is not None and target in callee_returns:
            return callee_returns[target]
        return None

    immediate = re.fullmatch(r"\$0x([0-9a-fA-F]+),%eax", operands)
    if immediate:
        value = int(immediate.group(1), 16)
        if mnemonic.startswith("mov"):
            return frozenset((value & 0xFFFFFFFF,))
        if mnemonic.startswith("add"):
            return map_values(values, lambda old: old + value)
        if mnemonic.startswith("sub"):
            return map_values(values, lambda old: old - value)
        if mnemonic.startswith("and"):
            return map_values(values, lambda old: old & value)
        if mnemonic.startswith("or"):
            return map_values(values, lambda old: old | value)
        if mnemonic.startswith("xor"):
            return map_values(values, lambda old: old ^ value)

    if operands == "%eax":
        if mnemonic.startswith("inc"):
            return map_values(values, lambda old: old + 1)
        if mnemonic.startswith("dec"):
            return map_values(values, lambda old: old - 1)
        if mnemonic.startswith("neg"):
            return map_values(values, lambda old: -old)
        if mnemonic.startswith("not"):
            return map_values(values, lambda old: ~old)
        if mnemonic.startswith("pop"):
            return None
    if operands == "%eax,%eax" and mnemonic.startswith("sbb"):
        return frozenset((0, 0xFFFFFFFF))

    # A byte-sized result is exact when the known high 24 bits are zero. `setcc` is boolean;
    # movb's literal preserves those high bits.
    byte_immediate = re.fullmatch(r"\$0x([0-9a-fA-F]+),%al", operands)
    if byte_immediate and mnemonic.startswith("mov") and values is not None:
        byte = int(byte_immediate.group(1), 16) & 0xFF
        return frozenset((value & 0xFFFFFF00) | byte for value in values)
    if operands == "%al" and mnemonic.startswith("set") and values is not None:
        high = {value & 0xFFFFFF00 for value in values}
        if len(high) <= 16:
            return frozenset(part | bit for part in high for bit in (0, 1))

    destination = operands.rsplit(",", 1)[-1] if operands else ""
    if destination in ("%eax", "%ax", "%al", "%ah"):
        # Comparisons have no destination despite AT&T's final-operand spelling.
        if not mnemonic.startswith(("cmp", "test")):
            return None
    if mnemonic.startswith(
        (
            "mul",
            "imul",
            "div",
            "idiv",
            "lods",
            "cpuid",
            "rdtsc",
            "rdmsr",
            "cmpxchg",
            "xlat",
        )
    ) or mnemonic in (
        "cbw",
        "cwde",
        "lahf",
        "popa",
        "popal",
        "aaa",
        "aad",
        "aam",
        "aas",
        "daa",
        "das",
    ):
        return None
    if mnemonic.startswith("xchg") and "%eax" in operands:
        return None
    return values


def function_layout(instructions, bindings):
    by_va = {instruction["va"]: index for index, instruction in enumerate(instructions)}
    text_end = instructions[-1]["va"] + len(instructions[-1]["bytes"])
    starts = sorted(
        start for start in function_starts(instructions, bindings) if start in by_va
    )
    ends = {
        start: starts[index + 1] if index + 1 < len(starts) else text_end
        for index, start in enumerate(starts)
    }
    return by_va, starts, ends


def function_calls(instructions, by_va, function, end):
    """Direct call targets in one conservative function extent, including unreachable blocks."""
    result = set()
    index = by_va.get(function)
    while index is not None and index < len(instructions):
        instruction = instructions[index]
        if instruction["va"] >= end:
            break
        if instruction["bytes"][:1] == b"\xe8":
            target = direct_target(instruction)
            if target is not None:
                result.add(target)
        index += 1
    return result


def validate_lua_api_map(instructions, bindings, by_va, ends):
    """Fail if the build-local Lua entry-point calibration no longer matches its anchors."""
    baselib = {
        name: function
        for name, function, _pair_va, _table_va, kind in bindings
        if kind == "baselib"
    }
    expected = {
        "type": {LUAL_CHECKANY, LUA_TYPE, LUA_TYPENAME, LUA_PUSHSTRING},
        "rawequal": {LUAL_CHECKANY, LUA_RAWEQUAL, LUA_PUSHBOOLEAN},
        "rawget": {LUAL_CHECKTYPE, LUAL_CHECKANY, LUA_SETTOP, LUA_RAWGET},
        "rawset": {LUAL_CHECKTYPE, LUAL_CHECKANY, LUA_SETTOP, LUA_RAWSET},
        "assert": {LUAL_CHECKANY, LUA_TOBOOLEAN, LUA_GETTOP},
    }
    for name, required in expected.items():
        function = baselib.get(name)
        if function is None or function not in ends:
            sys.exit(f"could not locate base-library calibration binding {name!r}")
        calls = function_calls(instructions, by_va, function, ends[function])
        missing = required - calls
        if missing:
            sys.exit(
                f"build-{BUILD} Lua API calibration failed in {name}: missing calls "
                f"{[hex(value) for value in sorted(missing)]}"
            )


def analyse_function_returns(
    instructions, by_va, function, end, callee_returns=None, arity_limit=None
):
    """Collect EAX constants at every reachable return and explain any unresolved edge."""
    start_index = by_va.get(function)
    if start_index is None:
        return {"values": None, "reasons": {"arity-start-missing"}, "visited": frozenset()}

    callee_returns = callee_returns or {}
    states = {start_index: None}
    queue = collections.deque((start_index,))
    returns = frozenset()
    reasons = set()
    visited = set()
    iterations = 0
    while queue:
        index = queue.popleft()
        iterations += 1
        if iterations > 20_000:
            reasons.add("arity-iteration-cap")
            break
        instruction = instructions[index]
        if not function <= instruction["va"] < end:
            reasons.add("arity-cfg-escape")
            continue
        visited.add(index)
        values = eax_after(instruction, states[index], callee_returns)
        mnemonic = instruction["mnemonic"].lower()
        if mnemonic.startswith("ret"):
            if values is None:
                reasons.add("arity-return-unknown")
            else:
                merged = merge_values(returns, values)
                if merged is None:
                    reasons.add("arity-value-set-cap")
                else:
                    returns = merged
            continue

        successors = []
        if mnemonic in ("jmp", "jmpw", "jmpl", "ljmp"):
            target = branch_target(instruction)
            if target is None:
                reasons.add("arity-indirect-branch")
                continue
            if not function <= target < end:
                tail_values = callee_returns.get(target)
                if tail_values is None:
                    reasons.add("arity-cfg-escape")
                else:
                    merged = merge_values(returns, tail_values)
                    if merged is None:
                        reasons.add("arity-value-set-cap")
                    else:
                        returns = merged
                continue
            if target not in by_va:
                reasons.add("arity-cfg-escape")
                continue
            successors.append(by_va[target])
        elif mnemonic.startswith("j") or mnemonic.startswith("loop"):
            target = branch_target(instruction)
            if target is None:
                reasons.add("arity-indirect-branch")
                continue
            if not function <= target < end or target not in by_va:
                reasons.add("arity-cfg-escape")
                continue
            successors.append(by_va[target])
            if index + 1 >= len(instructions) or instructions[index + 1]["va"] >= end:
                reasons.add("arity-cfg-escape")
                continue
            successors.append(index + 1)
        else:
            if index + 1 >= len(instructions) or instructions[index + 1]["va"] >= end:
                reasons.add("arity-cfg-escape")
                continue
            successors.append(index + 1)

        for successor in successors:
            if successor not in states:
                states[successor] = values
                queue.append(successor)
                continue
            old = states[successor]
            merged = merge_values(old, values)
            if merged != old:
                states[successor] = merged
                queue.append(successor)

    if not returns:
        reasons.add("arity-no-return")
    if arity_limit is not None and any(value > arity_limit for value in returns):
        reasons.add("arity-out-of-range")
    return {
        "values": returns if returns and not reasons else None,
        "reasons": reasons,
        "visited": frozenset(visited),
    }


def helper_return_summaries(instructions, by_va, ends, bindings):
    """Resolve EAX-return summaries for binding callees, to a conservative depth."""
    candidates = {row[1] for row in bindings if row[1] in ends}
    frontier = set(candidates)
    for _depth in range(3):
        next_frontier = set()
        for function in frontier:
            for target in function_calls(instructions, by_va, function, ends[function]):
                if target in ends and target not in candidates:
                    candidates.add(target)
                    next_frontier.add(target)
        frontier = next_frontier
        if not frontier:
            break

    summaries = {}
    for _round in range(6):
        changed = False
        for function in sorted(candidates, reverse=True):
            analysis = analyse_function_returns(
                instructions, by_va, function, ends[function], summaries
            )
            values = analysis["values"]
            if values is not None and summaries.get(function) != values:
                summaries[function] = values
                changed = True
        if not changed:
            break
    return summaries


def signed_32(value):
    return value - 0x100000000 if value & 0x80000000 else value


def binding_argc(instructions, visited):
    maximum = 0
    dynamic = False
    delegated = False
    variadic = False
    for index in sorted(visited):
        instruction = instructions[index]
        if not instruction["mnemonic"].lower().startswith("call"):
            continue
        target = direct_target(instruction)
        if target == LUA_GETTOP:
            variadic = True
        positions = LUA_INDEX_ARGUMENTS.get(target)
        if positions:
            pushes = instruction_pushes(instructions, index, max(positions) + 1)
            for position in positions:
                value = pushes[position]
                if value is None:
                    dynamic = True
                    continue
                value = signed_32(value)
                if value > 0:
                    maximum = max(maximum, value)
            continue
        if target not in LUA_KNOWN_TARGETS:
            delegated = True
    confidence = "lower-bound" if dynamic or delegated else "exact"
    notes = set()
    if dynamic:
        notes.add("argc-dynamic-index")
    if delegated:
        notes.add("delegated-argc")
    if variadic:
        notes.add("variadic")
    return maximum, confidence, notes


def kind_successors(instructions, by_va, function, end, index, reasons):
    instruction = instructions[index]
    mnemonic = instruction["mnemonic"].lower()
    if mnemonic.startswith("ret"):
        return []
    if mnemonic in ("jmp", "jmpw", "jmpl", "ljmp"):
        target = branch_target(instruction)
        if target is None:
            reasons.add("kinds-indirect-branch")
            return []
        if not function <= target < end or target not in by_va:
            reasons.add("kinds-cfg-escape")
            return []
        return [by_va[target]]
    if mnemonic.startswith("j") or mnemonic.startswith("loop"):
        target = branch_target(instruction)
        if target is None or not function <= target < end or target not in by_va:
            reasons.add("kinds-cfg-escape")
            return []
        if index + 1 >= len(instructions) or instructions[index + 1]["va"] >= end:
            reasons.add("kinds-cfg-escape")
            return []
        return [by_va[target], index + 1]
    if index + 1 >= len(instructions) or instructions[index + 1]["va"] >= end:
        reasons.add("kinds-cfg-escape")
        return []
    return [index + 1]


def binding_return_kinds(instructions, by_va, function, end, callee_returns):
    """Trace a known suffix of Lua pushes independently from the EAX arity proof."""
    start = by_va.get(function)
    if start is None:
        return None, "unknown", {"kinds-start-missing"}
    # State is (EAX constants, known stack suffix). Keeping path states separate preserves the
    # correlation between a return count and its returned kind tuple.
    initial = (None, ())
    states = {start: {initial}}
    queue = collections.deque((start,))
    returned = set()
    reasons = set()
    delegated_any = False
    iterations = 0
    while queue:
        index = queue.popleft()
        iterations += 1
        if iterations > 40_000:
            reasons.add("kinds-iteration-cap")
            break
        instruction = instructions[index]
        if not function <= instruction["va"] < end:
            reasons.add("kinds-cfg-escape")
            continue
        mnemonic = instruction["mnemonic"].lower()
        next_states = set()
        for eax_values, suffix in states[index]:
            values = eax_after(instruction, eax_values, callee_returns)
            next_suffix = suffix
            if mnemonic.startswith("call"):
                target = direct_target(instruction)
                if target in LUA_PUSH_KINDS:
                    next_suffix = suffix + (LUA_PUSH_KINDS[target],)
                elif target in LUA_REPLACE_TOP_KINDS:
                    kind = LUA_REPLACE_TOP_KINDS[target]
                    next_suffix = suffix[:-1] + (kind,) if suffix else (kind,)
                elif target in LUA_STACK_RESETTERS:
                    next_suffix = ()
                elif target in LUA_STACK_PRESERVING:
                    pass
                else:
                    next_suffix = ()
                    delegated_any = True
            if len(next_suffix) > 64:
                reasons.add("kinds-stack-cap")
                continue
            if mnemonic.startswith("ret"):
                if values is None:
                    reasons.add("kinds-return-unknown")
                    continue
                for arity in values:
                    if arity > 64 or len(next_suffix) < arity:
                        reasons.add("kinds-trace-mismatch")
                        continue
                    returned.add(next_suffix[-arity:] if arity else ())
                    if len(next_suffix) > arity:
                        reasons.add("stack-leak")
                continue
            next_states.add((values, next_suffix))

        if mnemonic.startswith("ret"):
            continue
        successors = kind_successors(instructions, by_va, function, end, index, reasons)
        for successor in successors:
            old = states.setdefault(successor, set())
            merged = old | next_states
            if len(merged) > 64:
                reasons.add("kinds-state-cap")
                continue
            if merged != old:
                states[successor] = merged
                queue.append(successor)

    notes = set(reasons)
    if delegated_any:
        notes.add("delegated-push")
    fatal = reasons - {"stack-leak"}
    if not returned or fatal:
        return None, "unverified", notes
    return frozenset(returned), "agree", notes


def binding_shapes(instructions, bindings):
    by_va, _starts, ends = function_layout(instructions, bindings)
    validate_lua_api_map(instructions, bindings, by_va, ends)
    callee_returns = helper_return_summaries(instructions, by_va, ends, bindings)
    result = {}
    for function in sorted({row[1] for row in bindings}):
        end = ends.get(function)
        if end is None:
            arity_analysis = {
                "values": None,
                "reasons": {"arity-start-missing"},
                "visited": frozenset(),
            }
        else:
            arity_analysis = analyse_function_returns(
                instructions,
                by_va,
                function,
                end,
                callee_returns,
                arity_limit=64,
            )
        baseline = (
            analyse_function_returns(instructions, by_va, function, end, arity_limit=64)
            if end is not None
            else arity_analysis
        )
        notes = set(arity_analysis["reasons"])
        if arity_analysis["values"] is not None and baseline["values"] is None:
            notes.add("arity-helper-summary")
        argc, argc_conf, argc_notes = binding_argc(
            instructions, arity_analysis["visited"] or baseline["visited"]
        )
        if arity_analysis["reasons"] & {
            "arity-start-missing",
            "arity-cfg-escape",
            "arity-indirect-branch",
            "arity-iteration-cap",
        }:
            argc_conf = "lower-bound"
            argc_notes.add("argc-cfg-partial")
        notes.update(argc_notes)
        if end is None:
            kinds, kinds_conf, kind_notes = None, "unknown", {"kinds-start-missing"}
        else:
            kinds, kinds_conf, kind_notes = binding_return_kinds(
                instructions, by_va, function, end, callee_returns
            )
        notes.update(kind_notes)
        result[function] = {
            "argc": argc,
            "argc_conf": argc_conf,
            "arity": arity_analysis["values"],
            "kinds": kinds,
            "kinds_conf": kinds_conf,
            "notes": notes,
        }
    return result


def validate_shape_anchors(bindings, shapes):
    """Keep independently recognisable WotLK signatures and analysis coverage from regressing."""
    expected_arities = {
        "GetBuildInfo": collections.Counter({(4,): 1, (5,): 1}),
        "GetAddOnInfo": collections.Counter({(0, 7): 1, (0, 8): 1}),
        "GetAuctionItemInfo": collections.Counter({(0, 13): 1}),
        "UnitExists": collections.Counter({(1,): 1}),
        "CreateFrame": collections.Counter({(0, 1): 1}),
    }
    for name, expected in expected_arities.items():
        actual = collections.Counter(
            tuple(sorted(shapes[function]["arity"] or ()))
            for binding_name, function, _pair_va, _table_va, _kind in bindings
            if binding_name == name
        )
        if actual != expected:
            sys.exit(f"shape anchor {name} changed: got {actual}, expected {expected}")

    baselib = {
        name: shapes[function]
        for name, function, _pair_va, _table_va, kind in bindings
        if kind == "baselib"
    }
    expected_base = {
        "type": (1, frozenset((1,)), frozenset((("string?",),))),
        "rawequal": (2, frozenset((1,)), frozenset((("boolean",),))),
        "rawget": (2, frozenset((1,)), frozenset((("any",),))),
    }
    for name, (argc, arity, kinds) in expected_base.items():
        shape = baselib.get(name)
        actual = None if shape is None else (shape["argc"], shape["arity"], shape["kinds"])
        if actual != (argc, arity, kinds):
            sys.exit(
                f"base-library shape anchor {name} changed: got {actual!r}, expected "
                f"{(argc, arity, kinds)!r}"
            )

    arities = sum(shapes[row[1]]["arity"] is not None for row in bindings)
    kinds = sum(shapes[row[1]]["kinds_conf"] == "agree" for row in bindings)
    if arities < 2_828 or kinds < 2_445:
        sys.exit(
            f"shape-analysis coverage regressed: {arities} arities/{kinds} kinds, expected at "
            "least 2828/2445"
        )


def formatted_kinds(kinds):
    return " | ".join(
        "(" + ",".join(values) + ")" for values in sorted(kinds)
    )


def write_shapes(path, tables, rows, shapes):
    name_counts = collections.Counter(row[0] for row in rows)
    duplicate_names = {name for name, count in name_counts.items() if count > 1}
    analysed = sum(shapes[row[1]]["arity"] is not None for row in rows)
    kinds_analysed = sum(shapes[row[1]]["kinds_conf"] == "agree" for row in rows)
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# Lua binding registry of the {VERSION} client (WoW.exe build {BUILD}): exact "
            "names, functions and table membership.\n"
            "#\n"
            f"# {len(tables)} registrar tables, {len(rows)} bindings, {analysed} with CFG-proven "
            "return arity. The 73 "
            "RegisterFunction loop\n"
            "# sites at 0x817f90 supply exact global table extents; 39 calls to 0x8167e0 supply "
            "exact widget\n"
            "# method extents; seven null-terminated `.rdata` tables are the FrameScript base and "
            "Lua 5.1 libraries.\n"
            "#\n"
            f"# All rows have an observed argc floor and {kinds_analysed} have an independent "
            "return-kind trace. Arity is\n"
            "# exact when every reachable ret has one constant, a sound `|`-set when paths "
            "differ, and unknown\n"
            "# when EAX or control flow cannot be proved. argc is never gateable: even `exact` "
            "means only that every\n"
            "# stack-index API observed in this body used a constant; an opaque helper makes it "
            "a `lower-bound`.\n"
            "# kinds_conf=agree means the known Lua-stack suffix covers every returned slot. "
            "`delegated-push` rows\n"
            "# describe only the binding's own visible stack effects and remain advisory.\n"
            "name\tfn\tpair_va\ttable_va\ttable_kind\targc\targc_conf\tarity\t"
            "arity_conf\tkinds\tkinds_conf\tnotes\n"
        )
        for name, function, pair_va, table_va, table_kind in sorted(
            rows, key=lambda row: (row[0], row[3], row[2])
        ):
            shape = shapes[function]
            arity_values = shape["arity"]
            if arity_values is None:
                arity = "?"
                arity_conf = "unknown"
            else:
                arity = "|".join(str(value) for value in sorted(arity_values))
                arity_conf = "exact" if len(arity_values) == 1 else "set"
            kinds = "?" if shape["kinds"] is None else formatted_kinds(shape["kinds"])
            notes = sorted(shape["notes"])
            if name in duplicate_names:
                notes.append("name-not-unique")
            out.write(
                f"{name}\t0x{function:x}\t0x{pair_va:x}\t0x{table_va:x}\t{table_kind}\t"
                f"{shape['argc']}\t{shape['argc_conf']}\t{arity}\t{arity_conf}\t{kinds}\t"
                f"{shape['kinds_conf']}\t{';'.join(notes)}\n"
            )


def message_catalog(image):
    rows = []
    for message_id in range(MESSAGE_COUNT):
        row_va = MESSAGE_TABLE + MESSAGE_ROW_SIZE * message_id
        key_pointer, kind, sound_pointer, type_tag, chat_type = struct.unpack(
            "<IIIII", image.bytes_at(row_va, MESSAGE_ROW_SIZE)
        )
        try:
            key = image.cstring(key_pointer)
            sound = image.cstring(sound_pointer)
        except (UnicodeDecodeError, ValueError) as error:
            sys.exit(f"message row {message_id:#x} has a bad string pointer: {error}")
        if not re.fullmatch(r"[A-Z][A-Z0-9_]*", key):
            sys.exit(f"message row {message_id:#x} has invalid key {key!r}")
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", sound):
            sys.exit(f"message row {message_id:#x} has invalid sound {sound!r}")
        if kind > 3:
            sys.exit(f"message row {message_id:#x} has invalid kind {kind}")
        rows.append((message_id, row_va, key, kind, sound, type_tag, chat_type))

    expected = {
        0x000: "ERR_INV_FULL",
        0x009: "ERR_WRONG_SLOT",
        0x00B: "ERR_BAG_FULL",
        MESSAGE_COUNT - 1: "ERR_BN_BROADCAST_THROTTLE",
    }
    for message_id, key in expected.items():
        if rows[message_id][2] != key:
            sys.exit(
                f"message {message_id:#x} is {rows[message_id][2]!r}, expected {key!r}"
            )
    return rows


def write_message_catalog(path, rows):
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# build {BUILD} CGGameUI message catalog (the {MESSAGE_TABLE:#x} table indexed by "
            "0x5216f0).\n"
            f"# {len(rows)} rows, ids 0x0..{len(rows) - 1:#x}; consumer bound "
            f"`cmp id,{MESSAGE_COUNT:#x}; jge` at 0x5216fc.\n"
            "# kind 0 -> 0x509dd0(text, chat_type); 1 -> UI_INFO_MESSAGE 0xbc; "
            "2 -> UI_ERROR_MESSAGE 0xbb;\n"
            "# kind 3 -> console 0x765360. No build-12340 row currently carries kind 3.\n"
            "id_hex\tid_dec\trow_va\tkey\tkind\tsound\ttype_tag\tchat_type\n"
        )
        for message_id, row_va, key, kind, sound, type_tag, chat_type in rows:
            out.write(
                f"0x{message_id:x}\t{message_id}\t0x{row_va:x}\t{key}\t{kind}\t{sound}\t"
                f"0x{type_tag:x}\t{chat_type}\n"
            )


def opcode_registrations(image, instructions, firesites):
    """Recover every literal incoming-opcode registration and its handler.

    The registration calls are intentionally the primary surface. Symbolic opcode names are not
    embedded beside this table, so external emulator/client enums must not silently become client
    evidence. A name is emitted only for the handler-event anchors above.
    """
    byte_anchors = {
        0x00631D89: bytes.fromhex("68 7c 14 00 00"),  # handler array byte extent
        0x00631DA2: bytes.fromhex("68 7c 14 00 00"),  # context array byte extent
        0x00631FA9: bytes.fromhex("89 94 81 3c 05 00 00"),
        0x00631FB3: bytes.fromhex("89 94 81 b8 19 00 00"),
        0x0063200B: bytes.fromhex("81 fe 1f 05 00 00"),
        0x00632013: bytes.fromhex("8b 84 b7 3c 05 00 00"),
        0x00632021: bytes.fromhex("8b 94 b7 b8 19 00 00"),
        0x0063202C: bytes.fromhex("ff d0"),
    }
    for va, expected in byte_anchors.items():
        actual = image.bytes_at(va, len(expected))
        if actual != expected:
            sys.exit(
                f"opcode mechanism anchor {va:#x} changed: got {actual.hex(' ')}, "
                f"expected {expected.hex(' ')}"
            )
    if 4 * OPCODE_COUNT != 0x147C:
        raise AssertionError("opcode table extent constant is inconsistent")

    text = image.section(".text")
    text_end = text["va"] + text["raw_size"]
    rows = []
    wrapper_calls = 0
    direct_calls = 0
    for index, instruction in enumerate(instructions):
        target = direct_target(instruction)
        if target == OPCODE_REGISTER_WRAPPER:
            wrapper_calls += 1
            path = "wrapper"
        elif (
            target == OPCODE_REGISTER
            and instruction["va"] != OPCODE_REGISTER_WRAPPER_CALL
        ):
            direct_calls += 1
            path = "direct"
        else:
            continue

        # A subsystem's first registration may push its arguments before a long block of harmless
        # global initialisation stores (0x52d0e5..0x52d1a4 is the longest build-12340 example).
        opcode, handler, context = instruction_pushes(instructions, index, 3, lookback=128)
        if opcode is None or opcode >= OPCODE_COUNT:
            sys.exit(
                f"opcode registration {instruction['va']:#x} has invalid literal opcode "
                f"{opcode!r}"
            )
        if handler is None or not text["va"] <= handler < text_end:
            sys.exit(
                f"opcode registration {instruction['va']:#x} has invalid handler {handler!r}"
            )
        rows.append(
            {
                "opcode": opcode,
                "registration": instruction["va"],
                "path": path,
                "handler": handler,
                "context": context,
            }
        )

    if (wrapper_calls, direct_calls, len(rows)) != (597, 14, 611):
        sys.exit(
            "opcode registration xrefs changed: "
            f"{wrapper_calls} wrapper, {direct_calls} direct, {len(rows)} decoded; "
            "expected 597, 14, 611"
        )

    # Attribute only direct event calls that lie in a registered handler's conservative function
    # extent. A shared handler may serve several opcodes, so this is handler evidence rather than a
    # claim that every listed event fires on every path through that handler.
    fake_bindings = [
        ("", handler, 0, 0, "opcode") for handler in {row["handler"] for row in rows}
    ]
    _by_va, starts, ends = function_layout(instructions, fake_bindings)
    handlers = {row["handler"] for row in rows}
    handler_events = collections.defaultdict(set)
    for site_va, _dispatcher, event_id, event_name, _arg_format in firesites:
        if event_id is None or not event_name:
            continue
        position = bisect.bisect_right(starts, site_va)
        if not position:
            continue
        function = starts[position - 1]
        if function in handlers and site_va < ends[function]:
            handler_events[function].add((event_name, site_va))

    registrations = collections.defaultdict(set)
    for row in rows:
        registrations[row["opcode"]].add(row["handler"])
        row["events"] = tuple(sorted(handler_events.get(row["handler"], ())))
        row["name"] = ""
        row["name_conf"] = "unknown"

    firesite_lookup = {
        (site_va, event_name)
        for site_va, _dispatcher, event_id, event_name, _arg_format in firesites
        if event_id is not None and event_name
    }
    for opcode, (handler, fire_site, event_name) in OPCODE_EVENT_NAME_ANCHORS.items():
        if registrations.get(opcode) != {handler}:
            sys.exit(
                f"opcode anchor {opcode:#x} handlers are "
                f"{sorted(hex(value) for value in registrations.get(opcode, ()))}, "
                f"expected only {handler:#x}"
            )
        if (fire_site, event_name) not in firesite_lookup:
            sys.exit(
                f"opcode anchor {opcode:#x} handler {handler:#x} no longer has "
                f"{event_name} at {fire_site:#x}"
            )
        if (event_name, fire_site) not in handler_events.get(handler, set()):
            sys.exit(
                f"opcode anchor {opcode:#x} fire site {fire_site:#x} was not attributed to "
                f"handler {handler:#x}"
            )
        for row in rows:
            if row["opcode"] == opcode and row["handler"] == handler:
                row["name"] = f"SMSG_{event_name}"
                row["name_conf"] = "handler-event"

    return sorted(rows, key=lambda row: (row["opcode"], row["registration"]))


def write_opcodes(path, rows):
    named = sum(bool(row["name"]) for row in rows)
    handlers = len({row["handler"] for row in rows})
    opcodes = len({row["opcode"] for row in rows})
    with_events = sum(bool(row["events"]) for row in rows)
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# Incoming world-opcode registrations of the {VERSION} client "
            f"(WoW.exe build {BUILD}).\n"
            f"# The dispatcher at 0x63200b accepts ids below {OPCODE_COUNT:#x}, indexes the "
            "handler array at session+0x53c,\n"
            "# and calls it at 0x63202c. The parallel context array is at session+0x19b8. "
            "Registration function\n"
            f"# {OPCODE_REGISTER:#x} fills both arrays; most callers use wrapper "
            f"{OPCODE_REGISTER_WRAPPER:#x}.\n"
            "#\n"
            "# One row is one literal registration callsite. Repeated opcode ids are retained: "
            "they describe real lifecycle\n"
            "# registrations rather than a guessed final table. `context=dyn` means the context "
            "was pushed from a register.\n"
            "# `direct_events` lists literal-id FrameScript fires inside the registered handler's "
            "conservative function\n"
            "# extent. Shared handlers can serve several opcodes, so these are handler facts, not "
            "unconditional packet effects.\n"
            "# Symbolic names are blank unless the exact handler directly proves the name through "
            "a matching event. No server\n"
            "# or other-client opcode enum fills the blanks. `handler-event` is the only current "
            "name confidence.\n"
            f"# {len(rows)} registrations; {opcodes} opcode ids; {handlers} handlers; "
            f"{with_events} rows with direct events; {named} named rows.\n"
            "opcode_hex\topcode_dec\tregistration_va\tregistered_via\thandler_va\tcontext\t"
            "direct_events\tname\tname_conf\n"
        )
        for row in rows:
            events = "|".join(
                f"{event}@0x{site:x}" for event, site in row["events"]
            )
            context = "dyn" if row["context"] is None else f"0x{row['context']:x}"
            out.write(
                f"0x{row['opcode']:x}\t{row['opcode']}\t0x{row['registration']:x}\t"
                f"{row['path']}\t0x{row['handler']:x}\t{context}\t{events}\t"
                f"{row['name']}\t{row['name_conf']}\n"
            )


def verb_events(instructions, bindings, firesites):
    """Attribute dispatcher sites proven to lie in a registered binding's own body.

    Helper attribution needs a complete call graph and verified extents. This conservative pass
    does not attempt it: direct-call targets, binding entries and post-padding addresses establish
    function starts, and only a site whose enclosing start is the binding entry is published.
    """
    starts = {row[1] for row in bindings}
    starts.add(instructions[0]["va"])
    after_padding = False
    ended = False
    for instruction in instructions:
        mnemonic = instruction["mnemonic"]
        if mnemonic == "int3" or (mnemonic.startswith("nop") and (ended or after_padding)):
            after_padding = True
            continue
        if after_padding:
            starts.add(instruction["va"])
            after_padding = False
        target = direct_target(instruction)
        if target is not None and instruction["bytes"][0] == 0xE8:
            starts.add(target)
        ended = mnemonic.startswith("ret")
    ordered = sorted(starts)

    verbs_by_function = collections.defaultdict(set)
    for name, function, _pair_va, _table_va, table_kind in bindings:
        if table_kind == "global" and not name.startswith("Get"):
            verbs_by_function[function].add(name)

    table = collections.defaultdict(set)
    for site_va, _dispatcher, event_id, event_name, _arg_format in firesites:
        if event_id is None or not event_name:
            continue
        position = bisect.bisect_right(ordered, site_va)
        if not position:
            continue
        function = ordered[position - 1]
        for verb in verbs_by_function.get(function, ()):
            table[(verb, event_name)].add(site_va)
    return table


def write_verb_events(path, table):
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# The {VERSION} client's FrameScript events proven to fire inside a registered Lua "
            "verb's own body.\n"
            "# Generated by scripts/gen-reference-wotlk.py from the exact binding entries, "
            "function boundaries and\n"
            "# literal-id dispatcher sites in the build-12340 executable.\n"
            "#\n"
            "# Only `shape=body` is emitted. Calls through helpers are intentionally absent until "
            "a complete call graph\n"
            "# and verified function extents exist. This table is a floor, not a census: a verb "
            "with no row may fire\n"
            "# an event, and a computed-id dispatch cannot be named here.\n"
            f"# {len(table)} pairs over {len({verb for verb, _event in table})} verbs and "
            f"{len({event for _verb, event in table})} events.\n"
            "verb\tevent\tshape\tvia\tsites\n"
        )
        for (verb, event), sites in sorted(table.items()):
            out.write(
                f"{verb}\t{event}\tbody\t-\t"
                f"{'|'.join(f'0x{site:x}' for site in sorted(sites))}\n"
            )


def dbc_tables(image, instructions, bindings, opcodes):
    """Recover every client DBC table: its object, loader, literal widths and direct readers.

    The loader set is anchored on the two width-check format strings, which each loader passes
    once; the startup list and the object vtables must then account for every loader exactly.
    Readers are a floor: a function counts when it names an address inside the table object, or
    passes a literal game-table slot to the lookup; a read through a computed pointer is not seen.
    Every game-table lookup passes a literal slot, so a gt table's readers are complete.
    """
    anchors = {
        0x00634E2A: bytes.fromhex("be b0 48 63 00"),  # mov esi,0x6348b0
        0x00634E2F: bytes.fromhex("e8 9c e9 ff ff"),  # call 0x6337d0
        DBC_LOAD_SLOT: bytes.fromhex("8b 01 ff 60 04"),
        0x007F6A91: bytes.fromhex("83 fe 30"),  # twelve adapter slots
        0x007F6996: bytes.fromhex("8b 0c 85 90 4f af 00"),
    }
    for va, expected in anchors.items():
        actual = image.bytes_at(va, len(expected))
        if actual != expected:
            sys.exit(
                f"DBC mechanism anchor {va:#x} changed: got {actual.hex(' ')}, "
                f"expected {expected.hex(' ')}"
            )
    if 4 * GAME_TABLE_COUNT != 0x30:
        raise AssertionError("game-table adapter extent constant is inconsistent")

    def u32(va):
        return struct.unpack("<I", image.bytes_at(va, 4))[0]

    data = image.section(".data")
    text = image.section(".text")
    by_va, starts, _ends = function_layout(instructions, bindings)

    def enclosing(va):
        return starts[bisect.bisect_right(starts, va) - 1]

    def loader_of(obj):
        if not data["va"] <= obj < data["va"] + data["raw_size"]:
            sys.exit(f"DBC object {obj:#x} is not in .data")
        loader = u32(u32(obj) + 4)
        if not text["va"] <= loader < text["va"] + text["raw_size"]:
            sys.exit(f"DBC object {obj:#x} has a non-code load slot {loader:#x}")
        return loader

    # Width checks: `push expected; push eax; call thunk; push eax; push format`.
    checks = collections.defaultdict(dict)
    for index, instruction in enumerate(instructions):
        value = push_value(instruction)
        if len(instruction["bytes"]) != 5 or value not in (
            DBC_COLUMNS_FORMAT,
            DBC_ROW_SIZE_FORMAT,
        ):
            continue
        expected, found, call, name = instructions[index - 4 : index]
        thunk = direct_target(call)
        width = push_value(expected)
        if (
            width is None
            or found["bytes"] != b"\x50"
            or name["bytes"] != b"\x50"
            or thunk is None
            or call["bytes"][0] != 0xE8
        ):
            sys.exit(f"DBC width check {instruction['va']:#x} has an unknown shape")
        kind = "columns" if value == DBC_COLUMNS_FORMAT else "row_size"
        loader = enclosing(instruction["va"])
        if kind in checks[loader]:
            sys.exit(f"DBC loader {loader:#x} checks {kind} twice")
        checks[loader][kind] = width
        if checks[loader].setdefault("thunk", thunk) != thunk:
            sys.exit(f"DBC loader {loader:#x} names two files")

    tables = {}
    for loader, check in checks.items():
        if set(check) != {"columns", "row_size", "thunk"}:
            sys.exit(f"DBC loader {loader:#x} lacks a width check: {sorted(check)}")
        raw = image.bytes_at(check["thunk"], 6)
        if raw[0] != 0xB8 or raw[5] != 0xC3:
            sys.exit(f"DBC name thunk {check['thunk']:#x} is not `mov eax,imm; ret`")
        path = image.cstring(struct.unpack_from("<I", raw, 1)[0])
        match = re.fullmatch(r"DBFilesClient\\(\w+)\.dbc", path)
        if not match:
            sys.exit(f"DBC loader {loader:#x} names {path!r}")
        if match.group(1) in tables:
            sys.exit(f"two DBC loaders read {match.group(1)}")
        tables[match.group(1)] = {
            "loader": loader,
            "columns": check["columns"],
            "row_size": check["row_size"],
        }
    by_loader = {row["loader"]: name for name, row in tables.items()}

    # Load sites: the startup run, then a direct `mov ecx,<object>; call <loader>` for the rest.
    loads = {}
    index = by_va[DBC_LOAD_ALL[0]]
    while instructions[index]["va"] < DBC_LOAD_ALL[1]:
        instruction = instructions[index]
        if instructions[index + 1]["bytes"] == b"\xff\xd6":  # call esi
            raw = instruction["bytes"]
            if len(raw) != 5 or raw[0] != 0xB9:
                sys.exit(f"DBC startup load {instruction['va']:#x} has no literal object")
            obj = struct.unpack_from("<I", raw, 1)[0]
            loads[obj] = (instruction["va"], "startup")
        index += 1
    for index, instruction in enumerate(instructions[:-1]):
        raw = instruction["bytes"]
        target = direct_target(instructions[index + 1])
        if len(raw) == 5 and raw[0] == 0xB9 and target in by_loader:
            obj = struct.unpack_from("<I", raw, 1)[0]
            if obj in loads:
                sys.exit(f"DBC object {obj:#x} is loaded twice")
            loads[obj] = (instruction["va"], f"0x{enclosing(instruction['va']):x}")

    for obj, (site, loaded_by) in loads.items():
        name = by_loader.get(loader_of(obj))
        if name is None:
            sys.exit(f"DBC object {obj:#x} loaded at {site:#x} has no checked loader")
        if "object" in tables[name]:
            sys.exit(f"{name} has two table objects")
        tables[name].update(object=obj, load_site=site, loaded_by=loaded_by)
    startup = sum(row.get("loaded_by") == "startup" for row in tables.values())
    unplaced = sorted(name for name, row in tables.items() if "object" not in row)
    if (len(tables), startup, unplaced) != (DBC_TABLE_COUNT, DBC_STARTUP_COUNT, []):
        sys.exit(
            f"DBC tables changed: {len(tables)} loaders, {startup} at startup, "
            f"unloaded {unplaced}; expected {DBC_TABLE_COUNT}, {DBC_STARTUP_COUNT}, none"
        )

    by_object = {row["object"]: name for name, row in tables.items()}
    game_tables = {}
    for slot in range(GAME_TABLE_COUNT):
        obj = u32(u32(GAME_TABLE_ADAPTERS + 4 * slot) + 0xC)
        name = by_object.get(obj)
        if name is None or not name.startswith("gt"):
            sys.exit(f"game-table slot {slot} reads {obj:#x}, not a gt table")
        tables[name]["game_table"] = slot
        game_tables[slot] = name
    if {name for name in tables if name.startswith("gt")} != set(game_tables.values()):
        sys.exit("a gt table has no game-table slot")

    # Code naming a table object: its load site, the static destructor that restores the
    # vtables (`mov dword [object or object+0x18], imm`), and every reader.
    rdata = image.section(".rdata")
    objects = sorted(by_object)
    load_sites = {row["load_site"] for row in tables.values()}
    hex_operand = re.compile(r"0x([0-9a-f]+)")
    named = collections.defaultdict(set)
    destructors = collections.defaultdict(set)
    for index, instruction in enumerate(instructions):
        raw = instruction["bytes"]
        if raw[:2] == b"\xc7\x05" and len(raw) == 10:
            target, value = struct.unpack_from("<II", raw, 2)
            position = bisect.bisect_right(objects, target)
            if (
                position
                and target - objects[position - 1] in (0, 0x18)
                and rdata["va"] <= value < rdata["va"] + rdata["raw_size"]
            ):
                destructors[by_object[objects[position - 1]]].add(enclosing(instruction["va"]))
                continue
        if instruction["va"] in load_sites:
            continue
        for operand in hex_operand.findall(instruction["operands"]):
            address = int(operand, 16)
            position = bisect.bisect_right(objects, address)
            if position and address < objects[position - 1] + DBC_OBJECT_SIZE:
                named[by_object[objects[position - 1]]].add(enclosing(instruction["va"]))
        if direct_target(instruction) == GAME_TABLE_LOOKUP and raw[0] == 0xE8:
            (slot,) = instruction_pushes(instructions, index, 1)
            if slot not in game_tables:
                sys.exit(f"game-table lookup {instruction['va']:#x} passes slot {slot!r}")
            named[game_tables[slot]].add(enclosing(instruction["va"]))

    lua = {}
    for binding_name, function, _pair, _table, kind in bindings:
        if kind in ("global", "method"):
            lua.setdefault(function, set()).add(
                binding_name if kind == "global" else f":{binding_name}"
            )
    handler_opcodes = collections.defaultdict(set)
    for row in opcodes:
        handler_opcodes[row["handler"]].add(row["opcode"])
    for name, row in tables.items():
        readers = named[name] - destructors[name] - {DBC_LOAD_ALL[0]}
        row["readers"] = sorted(readers)
        row["lua"] = sorted({entry for f in readers for entry in lua.get(f, ())})
        row["opcodes"] = sorted({op for f in readers for op in handler_opcodes.get(f, ())})
    return tables


def write_dbc_tables(path, tables):
    unread = sorted(name for name, row in tables.items() if not row["readers"])
    with open(path, "w", encoding="utf-8", newline="\n") as out:
        out.write(
            f"# Every DBC table the {VERSION} client loads (WoW.exe build {BUILD}), "
            "one row per table loader.\n"
            f"# {DBC_STARTUP_COUNT} load at startup through the run at "
            f"{DBC_LOAD_ALL[0]:#x} (`loaded_by=startup`); the rest are loaded by the named "
            "function.\n"
            "# columns / row_size are the loader's literal header checks: a file of any other "
            "width is a fatal error.\n"
            "# game_table is the slot a gt table is read through "
            f"({GAME_TABLE_LOOKUP:#x}(slot, row, column)); every lookup passes a literal "
            "slot.\n"
            "# readers are the functions naming an address inside the 0x24-byte table object "
            "(or passing its\n"
            "# literal game-table slot), past its load site and static destructor. They are a "
            "floor: a read through a\n"
            "# computed pointer is not seen. lua lists the Lua bindings (`:Name` a widget "
            "method) and opcodes the\n"
            "# incoming opcodes whose registered handler is itself a reader; a reader reached "
            "through a helper is\n"
            "# counted but not attributed.\n"
            f"# {len(tables)} tables; {len(unread)} with no reader: the client loads them "
            "and never reads them.\n"
            "table\tobject_va\tloader_va\tload_site_va\tloaded_by\tcolumns\trow_size\t"
            "game_table\treaders\tlua\topcodes\treader_vas\n"
        )
        for name in sorted(tables, key=str.lower):
            row = tables[name]
            slot = row.get("game_table")
            out.write(
                f"{name}\t0x{row['object']:x}\t0x{row['loader']:x}\t"
                f"0x{row['load_site']:x}\t{row['loaded_by']}\t{row['columns']}\t"
                f"{row['row_size']}\t{'-' if slot is None else slot}\t{len(row['readers'])}\t"
                f"{'|'.join(row['lua'])}\t"
                f"{'|'.join(f'0x{op:x}' for op in row['opcodes'])}\t"
                f"{'|'.join(f'0x{va:x}' for va in row['readers'])}\n"
            )


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, help=f"the {VERSION} build-{BUILD} Wow.exe")
    parser.add_argument("--out-dir", default=REFERENCE)
    parser.add_argument(
        "--objdump",
        default=shutil.which("objdump"),
        help="GNU objdump used to establish x86 instruction boundaries",
    )
    args = parser.parse_args()

    if not args.objdump:
        parser.error("--objdump is required (GNU objdump was not found on PATH)")

    image = checked_image(args.exe)
    rows, glue_rows = event_catalog(image)
    os.makedirs(args.out_dir, exist_ok=True)
    catalog_out = os.path.join(args.out_dir, "3.3.5a-event-catalog.tsv")
    write_event_catalog(catalog_out, rows)
    print(f"wrote {catalog_out}: {len(rows)} named slots of {EVENT_COUNT}")
    glue_out = os.path.join(args.out_dir, "3.3.5a-glue-events.tsv")
    write_glue_event_catalog(glue_out, glue_rows)
    print(f"wrote {glue_out}: {len(glue_rows)} glue slots")

    instructions = disassembly(image, args.objdump)
    firesites = event_firesites(image, instructions, rows)
    firesites_out = os.path.join(args.out_dir, "3.3.5a-event-firesites.tsv")
    write_event_firesites(firesites_out, firesites)
    print(f"wrote {firesites_out}: {len(firesites)} dispatcher callsites")

    token_fanout_ids = computed_event_families(instructions)
    events_out = os.path.join(args.out_dir, "3.3.5a-events.tsv")
    event_names, produced = write_events(events_out, rows, firesites, token_fanout_ids)
    print(
        f"wrote {events_out}: {event_names} events, {produced} with attributed producers; "
        f"{len(token_fanout_ids)} explicit unit-token fan-out ids"
    )

    tables, bindings = binding_tables(image, instructions)
    shapes = binding_shapes(instructions, bindings)
    validate_shape_anchors(bindings, shapes)
    shapes_out = os.path.join(args.out_dir, "3.3.5a-shapes.tsv")
    write_shapes(shapes_out, tables, bindings, shapes)
    analysed_arities = sum(shapes[row[1]]["arity"] is not None for row in bindings)
    analysed_kinds = sum(shapes[row[1]]["kinds_conf"] == "agree" for row in bindings)
    print(
        f"wrote {shapes_out}: {len(bindings)} bindings in {len(tables)} tables; "
        f"{analysed_arities} return arities and {analysed_kinds} kind traces proved"
    )

    messages = message_catalog(image)
    messages_out = os.path.join(args.out_dir, "3.3.5a-message-catalog.tsv")
    write_message_catalog(messages_out, messages)
    print(f"wrote {messages_out}: {len(messages)} message rows")

    opcodes = opcode_registrations(image, instructions, firesites)
    opcodes_out = os.path.join(args.out_dir, "3.3.5a-opcodes.tsv")
    write_opcodes(opcodes_out, opcodes)
    print(
        f"wrote {opcodes_out}: {len(opcodes)} incoming registrations over "
        f"{len({row['opcode'] for row in opcodes})} opcode ids"
    )

    verb_event_rows = verb_events(instructions, bindings, firesites)
    verb_events_out = os.path.join(args.out_dir, "3.3.5a-verb-events.tsv")
    write_verb_events(verb_events_out, verb_event_rows)
    print(f"wrote {verb_events_out}: {len(verb_event_rows)} verb/event pairs")

    dbc = dbc_tables(image, instructions, bindings, opcodes)
    dbc_out = os.path.join(args.out_dir, "3.3.5a-dbc.tsv")
    write_dbc_tables(dbc_out, dbc)
    print(
        f"wrote {dbc_out}: {len(dbc)} tables, "
        f"{sum(not row['readers'] for row in dbc.values())} never read"
    )


if __name__ == "__main__":
    main()
