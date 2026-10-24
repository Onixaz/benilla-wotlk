//! The chat-type colour registry behind `GetChatTypeIndex`, `ChangeChatColor` and
//! `ResetChatColors`: the reference's runtime table (`0xb746e8`), which the `COLORS` block of
//! `chat-cache.txt` overwrites in place at load; the app owns that file.

use mlua::{Lua, MultiValue, Value};

/// The fixed segment: 62 entries, in the reference's order (index = position + 1).
pub const FIXED_CHAT_TYPES: usize = 62;
/// The extras seeded at boot (`0x50ef6a`): `CHANNEL1` to `CHANNEL10`, indices 63 to 72.
pub const EXTRA_CHAT_TYPES: usize = 10;

/// The reference's defaults (`0x9fa558`, 0x4c-byte rows), verbatim; their order is the index
/// `GetChatTypeIndex` answers.
const DEFAULTS: [(&str, [u8; 3]); FIXED_CHAT_TYPES] = [
    ("SYSTEM", [255, 255, 0]),
    ("SAY", [255, 255, 255]),
    ("PARTY", [170, 170, 255]),
    ("RAID", [255, 127, 0]),
    ("GUILD", [64, 255, 64]),
    ("OFFICER", [64, 192, 64]),
    ("YELL", [255, 64, 64]),
    ("WHISPER", [255, 128, 255]),
    ("WHISPER_FOREIGN", [255, 128, 255]),
    ("WHISPER_INFORM", [255, 128, 255]),
    ("EMOTE", [255, 128, 64]),
    ("TEXT_EMOTE", [255, 128, 64]),
    ("MONSTER_SAY", [255, 255, 159]),
    ("MONSTER_PARTY", [170, 170, 255]),
    ("MONSTER_YELL", [255, 64, 64]),
    ("MONSTER_WHISPER", [255, 181, 235]),
    ("MONSTER_EMOTE", [255, 128, 64]),
    ("CHANNEL", [255, 192, 192]),
    ("CHANNEL_JOIN", [192, 128, 128]),
    ("CHANNEL_LEAVE", [192, 128, 128]),
    ("CHANNEL_LIST", [192, 128, 128]),
    ("CHANNEL_NOTICE", [192, 192, 192]),
    ("CHANNEL_NOTICE_USER", [192, 192, 192]),
    ("AFK", [255, 128, 255]),
    ("DND", [255, 128, 255]),
    ("IGNORED", [255, 0, 0]),
    ("SKILL", [85, 85, 255]),
    ("LOOT", [0, 170, 0]),
    ("MONEY", [255, 255, 0]),
    ("OPENING", [128, 128, 255]),
    ("TRADESKILLS", [255, 255, 255]),
    ("PET_INFO", [128, 128, 255]),
    ("COMBAT_MISC_INFO", [128, 128, 255]),
    ("COMBAT_XP_GAIN", [111, 111, 255]),
    ("COMBAT_HONOR_GAIN", [224, 202, 10]),
    ("COMBAT_FACTION_CHANGE", [128, 128, 255]),
    ("BG_SYSTEM_NEUTRAL", [255, 120, 10]),
    ("BG_SYSTEM_ALLIANCE", [0, 174, 239]),
    ("BG_SYSTEM_HORDE", [255, 0, 0]),
    ("RAID_LEADER", [255, 72, 9]),
    ("RAID_WARNING", [255, 72, 0]),
    ("RAID_BOSS_EMOTE", [255, 221, 0]),
    ("RAID_BOSS_WHISPER", [255, 221, 0]),
    ("FILTERED", [255, 0, 0]),
    ("BATTLEGROUND", [255, 127, 0]),
    ("BATTLEGROUND_LEADER", [255, 219, 183]),
    ("RESTRICTED", [255, 0, 0]),
    ("BATTLENET", [255, 255, 255]),
    ("ACHIEVEMENT", [255, 255, 0]),
    ("GUILD_ACHIEVEMENT", [64, 255, 64]),
    ("ARENA_POINTS", [255, 255, 255]),
    ("PARTY_LEADER", [118, 200, 255]),
    ("TARGETICONS", [255, 255, 0]),
    ("BN_WHISPER", [0, 255, 246]),
    ("BN_WHISPER_INFORM", [0, 255, 246]),
    ("BN_CONVERSATION", [0, 177, 240]),
    ("BN_CONVERSATION_NOTICE", [0, 177, 240]),
    ("BN_CONVERSATION_LIST", [0, 177, 240]),
    ("BN_INLINE_TOAST_ALERT", [130, 197, 255]),
    ("BN_INLINE_TOAST_BROADCAST", [130, 197, 255]),
    ("BN_INLINE_TOAST_BROADCAST_INFORM", [130, 197, 255]),
    ("BN_INLINE_TOAST_CONVERSATION", [130, 197, 255]),
];

/// The 1-based index of `CHANNEL`, the extras' colour source at seed and reset (`0xb74c34`).
const CHANNEL_INDEX: usize = 18;

/// One registry entry: the type's name and its live colour bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatTypeColor {
    pub name: String,
    pub rgb: [u8; 3],
}

/// A fixed type's shipped colour, matched as `GetChatTypeIndex` matches; `None` for a name the
/// reference does not carry.
pub fn default_chat_color(name: &str) -> Option<[u8; 3]> {
    DEFAULTS
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|&(_, rgb)| rgb)
}

/// The boot seed: the defaults, then the extras coloured from `CHANNEL` (`0x50edd0`).
pub(super) fn seed() -> Vec<ChatTypeColor> {
    let mut table: Vec<ChatTypeColor> = DEFAULTS
        .iter()
        .map(|(name, rgb)| ChatTypeColor {
            name: (*name).to_string(),
            rgb: *rgb,
        })
        .collect();
    let channel = table[CHANNEL_INDEX - 1].rgb;
    for n in 1..=EXTRA_CHAT_TYPES {
        table.push(ChatTypeColor {
            name: format!("CHANNEL{n}"),
            rgb: channel,
        });
    }
    table
}

/// `ResetChatColors`: the defaults back first, then the extras from the restored `CHANNEL`.
fn reset(table: &mut [ChatTypeColor]) {
    for (entry, (_, rgb)) in table.iter_mut().zip(DEFAULTS.iter()) {
        entry.rgb = *rgb;
    }
    let channel = table[CHANNEL_INDEX - 1].rgb;
    for entry in table.iter_mut().skip(FIXED_CHAT_TYPES) {
        entry.rgb = channel;
    }
}

/// ASCII case-insensitive and nothing wider: the reference's compare folds only `A` to `Z`
/// (`0x64a4c0`, `0x414310`).
fn position(table: &[ChatTypeColor], name: &str) -> Option<usize> {
    table.iter().position(|e| e.name.eq_ignore_ascii_case(name))
}

/// Truncates, never rounds: the reference's `fmul 255.0; __ftol` (`0x4a085f`-`0x4a08a3`).
fn to_byte(x: f64) -> u8 {
    if !x.is_finite() {
        return 0;
    }
    ((x * 255.0).trunc() as i64) as u8
}

/// `UPDATE_CHAT_COLOR`'s value: the stored byte times the f32 reciprocal of 255 (`0x8026c8`), so
/// an addon that passed `0.5` hears `127/255`.
fn from_byte(b: u8) -> f64 {
    f64::from(b as f32 * (1.0f32 / 255.0f32))
}

impl super::UiScript {
    /// Apply the `COLORS` block of `chat-cache.txt` as the loader does: matched entries
    /// overwritten in place, unknown names ignored.
    pub fn set_chat_colors(&mut self, colors: impl IntoIterator<Item = (String, [u8; 3])>) {
        let mut model = self.model_mut();
        for (name, rgb) in colors {
            if let Some(i) = position(&model.chat_colors, &name) {
                model.chat_colors[i].rgb = rgb;
            }
        }
    }

    /// The live registry, fixed segment first, then the extras.
    pub fn chat_colors(&self) -> Vec<ChatTypeColor> {
        self.model_mut().chat_colors.clone()
    }

    /// Whether Lua changed a colour since the last drain, the app's cue to persist.
    pub fn take_chat_color_changes(&mut self) -> bool {
        drain_field!(self, chat_colors_changed)
    }
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    let g = lua.globals();

    g.set(
        "GetChatTypeIndex",
        lua.create_function(|lua, name: Option<String>| {
            let Some(name) = name else {
                return Ok(0);
            };
            let model = lua
                .app_data_ref::<crate::script::ModelCell>()
                .expect("model app_data");
            Ok(position(&model.chat_colors, &name).map_or(0, |i| i as i64 + 1))
        })?,
    )?;

    g.set(
        "ChangeChatColor",
        lua.create_function(
            |lua, (name, r, g, b): (String, Option<f64>, Option<f64>, Option<f64>)| {
                let written = {
                    let mut model = crate::script::model_mut_of(lua);
                    let Some(i) = position(&model.chat_colors, &name) else {
                        return Ok(());
                    };
                    let rgb = [
                        to_byte(r.unwrap_or(0.0)),
                        to_byte(g.unwrap_or(0.0)),
                        to_byte(b.unwrap_or(0.0)),
                    ];
                    model.chat_colors[i].rgb = rgb;
                    model.chat_colors_changed = true;
                    (model.chat_colors[i].name.clone(), rgb)
                };
                let (name, rgb) = written;
                super::tick::fire_event_into(
                    lua,
                    "UPDATE_CHAT_COLOR",
                    vec![
                        super::ScriptValue::Str(name),
                        super::ScriptValue::Number(from_byte(rgb[0])),
                        super::ScriptValue::Number(from_byte(rgb[1])),
                        super::ScriptValue::Number(from_byte(rgb[2])),
                    ],
                );
                Ok(())
            },
        )?,
    )?;

    g.set(
        "ResetChatColors",
        lua.create_function(|lua, _ignored: MultiValue| {
            let mut model = crate::script::model_mut_of(lua);
            reset(&mut model.chat_colors);
            model.chat_colors_changed = true;
            Ok(Value::Nil)
        })?,
    )?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    #[test]
    fn the_index_is_one_based_case_folded_and_zero_on_a_miss() {
        let s = UiScript::new().unwrap();
        assert_eq!(
            s.eval::<i64>("return GetChatTypeIndex('system')").unwrap(),
            1
        );
        assert_eq!(s.eval::<i64>("return GetChatTypeIndex('say')").unwrap(), 2);
        assert_eq!(
            s.eval::<i64>("return GetChatTypeIndex('BN_INLINE_TOAST_CONVERSATION')")
                .unwrap(),
            62
        );
        assert_eq!(
            s.eval::<i64>("return GetChatTypeIndex('channel3')")
                .unwrap(),
            65,
            "extras are 63… — count_before_match + 0x3f"
        );
        assert_eq!(s.eval::<i64>("return GetChatTypeIndex('NOPE')").unwrap(), 0);
        assert_eq!(s.eval::<i64>("return GetChatTypeIndex()").unwrap(), 0);
    }

    #[test]
    fn change_chat_color_truncates_stores_and_fires_the_renormalised_bytes() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_COLOR') \
             f:SetScript('OnEvent', function(self, event, arg1, arg2, arg3, arg4, arg5, arg6, arg7, arg8, arg9) SEEN = {arg1, arg2, arg3, arg4} end) \
             ChangeChatColor('say', 0.5, 1.0, 0.999)",
        )
        .unwrap();
        let say = &s.chat_colors()[1];
        assert_eq!(say.name, "SAY");
        assert_eq!(say.rgb, [127, 255, 254], "truncate, never round");
        assert_eq!(
            s.eval::<String>("return SEEN[1]").unwrap(),
            "SAY",
            "the registry's spelling, not the caller's"
        );
        let heard = s.eval::<f64>("return SEEN[2]").unwrap();
        assert!(
            (heard - f64::from(127.0f32 * (1.0f32 / 255.0f32))).abs() < 1e-12,
            "the just-written byte re-normalised, not the 0.5 that was passed: {heard}"
        );
        assert!(s.take_chat_color_changes());
        assert!(!s.take_chat_color_changes(), "the cue is one-shot");
    }

    #[test]
    fn an_unknown_type_writes_nothing_and_fires_nothing() {
        let mut s = UiScript::new().unwrap();
        s.run(
            "local f = CreateFrame('Frame') f:RegisterEvent('UPDATE_CHAT_COLOR') \
             f:SetScript('OnEvent', function() SEEN = true end) \
             ChangeChatColor('NOPE', 0.5, 0.5, 0.5)",
        )
        .unwrap();
        assert!(s.eval::<bool>("return SEEN == nil").unwrap());
        assert!(!s.take_chat_color_changes());
    }

    #[test]
    fn the_cache_block_overwrites_in_place_and_reset_restores_the_seed() {
        let mut s = UiScript::new().unwrap();
        s.set_chat_colors([
            ("system".to_string(), [1, 2, 3]),
            ("CHANNEL".to_string(), [9, 9, 9]),
            ("BOGUS".to_string(), [7, 7, 7]),
        ]);
        let table = s.chat_colors();
        assert_eq!(table.len(), FIXED_CHAT_TYPES + EXTRA_CHAT_TYPES);
        assert_eq!(table[0].rgb, [1, 2, 3], "SYSTEM, matched case-folded");
        assert_eq!(
            table[FIXED_CHAT_TYPES].rgb,
            [255, 192, 192],
            "an extra keeps its seed colour — the cache does not re-seed it from CHANNEL"
        );
        s.run("ResetChatColors()").unwrap();
        let table = s.chat_colors();
        assert_eq!(table[0].rgb, [255, 255, 0]);
        assert_eq!(table[CHANNEL_INDEX - 1].rgb, [255, 192, 192]);
        assert_eq!(table[FIXED_CHAT_TYPES + 9].name, "CHANNEL10");
        assert!(s.take_chat_color_changes());
    }

    #[test]
    fn reset_recolours_the_extras_from_the_live_channel_entry() {
        let s = UiScript::new().unwrap();
        s.run("ChangeChatColor('CHANNEL', 0.0, 0.0, 0.0) ChangeChatColor('CHANNEL2', 1, 1, 1)")
            .unwrap();
        // The defaults come back first, so the extras take `CHANNEL`'s default, not the black.
        s.run("ResetChatColors()").unwrap();
        assert_eq!(s.chat_colors()[FIXED_CHAT_TYPES + 1].rgb, [255, 192, 192]);
    }
}
