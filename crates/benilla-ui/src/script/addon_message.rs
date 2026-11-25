//! `SendAddonMessage(prefix, message [, type [, target]])` (`0x500560`), the addon-to-addon lane:
//! one `CMSG_MESSAGECHAT` in language `-1` carrying `prefix`, a TAB and `message`, on the party,
//! raid, guild, battleground or whisper lane. The binding reads the arguments and raises the
//! verb's errors; the group gates and the packet run on the host inside the call.

use mlua::{Lua, Value};

/// The five lanes the verb sends on (`0x50062b`-`0x500645`): the chat types 2, 3, 4, `0x2c` and 7.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AddonDistribution {
    /// Chat type `0x02`; the default when the third argument is not a string (`0x5005f7`).
    Party,
    /// Chat type `0x03`; sent as PARTY outside a raid (`0x500663`).
    Raid,
    /// Chat type `0x04`.
    Guild,
    /// Chat type `0x2C`.
    Battleground,
    /// Chat type `0x07`, to the fourth argument's player (`0x50069b`).
    Whisper,
}

impl AddonDistribution {
    /// A Lua type token, matched ignoring case as the chat-type lookup does (`0x4fbb20`). `None`
    /// raises `Unknown addon chat type`, the one error for an unknown token and a type outside the
    /// five alike (`0x500647`).
    pub fn from_token(token: &str) -> Option<Self> {
        for (name, dist) in [
            ("PARTY", Self::Party),
            ("RAID", Self::Raid),
            ("GUILD", Self::Guild),
            ("BATTLEGROUND", Self::Battleground),
            ("WHISPER", Self::Whisper),
        ] {
            if token.eq_ignore_ascii_case(name) {
                return Some(dist);
            }
        }
        None
    }

    /// The lane's chat-type token.
    pub fn token(self) -> &'static str {
        match self {
            Self::Party => "PARTY",
            Self::Raid => "RAID",
            Self::Guild => "GUILD",
            Self::Battleground => "BATTLEGROUND",
            Self::Whisper => "WHISPER",
        }
    }
}

/// One addon message, composed at the binding as the reference's is, for the host's gates and
/// packet (`0x500678`-`0x50071f`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AddonSend {
    /// `prefix`, TAB, `message`, cut at 2999 bytes (`0x5005f0`), as bytes: the cut need not end
    /// on a UTF-8 boundary.
    pub text: Vec<u8>,
    /// The lane the caller named; the host applies the outside-a-raid downgrade.
    pub distribution: AddonDistribution,
    /// The whisper's player, never empty; `None` on the other lanes.
    pub target: Option<String>,
}

/// The compose buffer, 3000 bytes with its NUL (`0x5005ea`).
const ADDON_MESSAGE_BUFFER: usize = 0xbb8;

/// `lua_tolstring` with the verb's NULL-to-empty replacement (`0x5005ac`, `0x5005b5`): a string or
/// number's bytes, anything else empty.
fn lua_tostring(lua: &Lua, v: &Value) -> mlua::Result<Vec<u8>> {
    Ok(lua
        .coerce_string(v.clone())?
        .map(|s| s.as_bytes().to_vec())
        .unwrap_or_default())
}

pub(super) fn install(lua: &Lua) -> mlua::Result<()> {
    lua.globals().set(
        "SendAddonMessage",
        lua.create_function(
            |lua, (prefix, message, distribution, target): (Value, Value, Value, Value)| {
                // With no active player the call returns before any argument (`0x500586`).
                if super::calls::ask_host(lua, &super::HostAsk::ActivePlayer)
                    == Some(super::HostAnswer::Declined)
                {
                    return Ok(());
                }
                let prefix = lua_tostring(lua, &prefix)?;
                let message = lua_tostring(lua, &message)?;
                // Only both empty is the usage error (`0x5005bc`-`0x5005cc`).
                if prefix.is_empty() && message.is_empty() {
                    return Err(mlua::Error::RuntimeError(
                        r#"Usage: SendAddonMessage("prefix", "message" [,"type"] [,"target"])"#
                            .into(),
                    ));
                }
                // `"%s\t%s"` into the 3000-byte buffer (`0x5005f0`).
                let mut text = prefix;
                text.push(b'\t');
                text.extend_from_slice(&message);
                text.truncate(ADDON_MESSAGE_BUFFER - 1);
                // `lua_isstring` (`0x500600`) is true for a number, so `1` is the token "1" and
                // raises; anything else keeps PARTY.
                let distribution = match lua.coerce_string(distribution)? {
                    Some(token) => AddonDistribution::from_token(&token.to_string_lossy())
                        .ok_or_else(|| {
                            mlua::Error::RuntimeError("Unknown addon chat type".into())
                        })?,
                    None => AddonDistribution::Party,
                };
                // The fourth argument names the whisper's player; none or empty is the error
                // (`0x50069b`-`0x5006ae`).
                let target = if distribution == AddonDistribution::Whisper {
                    let name =
                        super::binding_abi::optional_string(lua, &target).filter(|t| !t.is_empty());
                    if name.is_none() {
                        return Err(mlua::Error::RuntimeError(
                            "SendAddonMessage(): Whisper message missing target player!".into(),
                        ));
                    }
                    name
                } else {
                    None
                };
                let send = AddonSend {
                    text,
                    distribution,
                    target,
                };
                // The group gates and the packet run on the host inside the call (`0x500678`,
                // `0x50071f`); with no host lent the VM records it, which only its tests read.
                if super::calls::ask_host(lua, &super::HostAsk::AddonSend(send.clone())).is_none() {
                    crate::script::model_mut_of(lua).addon_sends.push(send);
                }
                Ok(())
            },
        )?,
    )?;
    Ok(())
}

impl super::UiScript {
    /// Drain the messages `SendAddonMessage` recorded with no host lent.
    pub fn take_addon_sends(&mut self) -> Vec<AddonSend> {
        drain_field!(self, addon_sends)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::script::UiScript;

    fn sent_text(s: &mut UiScript) -> Vec<u8> {
        s.take_addon_sends().remove(0).text
    }

    #[test]
    fn a_message_is_prefix_tab_message_and_defaults_to_party() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("oRA", "hello")"#).unwrap();
        assert_eq!(
            s.take_addon_sends(),
            vec![AddonSend {
                text: b"oRA\thello".to_vec(),
                distribution: AddonDistribution::Party,
                target: None,
            }]
        );
        assert!(s.take_addon_sends().is_empty());
    }

    /// The lanes are the chat types 2, 3, 4, `0x2c` and 7 (`0x50062b`-`0x500645`); any other type,
    /// or a token naming none, is one error (`0x500647`).
    #[test]
    fn the_lanes_are_party_raid_guild_battleground_and_whisper() {
        let mut s = UiScript::new().unwrap();
        for (token, want) in [
            ("PARTY", AddonDistribution::Party),
            ("RAID", AddonDistribution::Raid),
            ("GUILD", AddonDistribution::Guild),
            ("BATTLEGROUND", AddonDistribution::Battleground),
            ("raid", AddonDistribution::Raid),
            ("Guild", AddonDistribution::Guild),
        ] {
            s.run(&format!(r#"SendAddonMessage("p", "m", "{token}")"#))
                .unwrap_or_else(|e| panic!("{token} must send: {e}"));
            let sent = s.take_addon_sends();
            assert_eq!(sent.len(), 1, "{token}");
            assert_eq!(sent[0].distribution, want, "{token}");
        }
        for token in [
            "SAY",
            "YELL",
            "EMOTE",
            "OFFICER",
            "CHANNEL",
            "AFK",
            "DND",
            "RAID_WARNING",
            "RAID_LEADER",
            "BATTLEGROUND_LEADER",
            "NOT_A_TYPE",
            "",
        ] {
            let err = s
                .run(&format!(r#"SendAddonMessage("p", "m", "{token}")"#))
                .expect_err(&format!("{token} must be refused"));
            assert!(
                err.to_string().contains("Unknown addon chat type"),
                "{token}: {err}"
            );
            assert!(s.take_addon_sends().is_empty(), "{token}");
        }
    }

    /// A whisper names its player in the fourth argument; none or empty is the verb's error
    /// (`0x5006a8`).
    #[test]
    fn a_whisper_needs_its_target() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("p", "m", "WHISPER", "Bob")"#)
            .unwrap();
        assert_eq!(
            s.take_addon_sends(),
            vec![AddonSend {
                text: b"p\tm".to_vec(),
                distribution: AddonDistribution::Whisper,
                target: Some("Bob".into()),
            }]
        );
        for args in [r#""WHISPER""#, r#""WHISPER", """#, r#""whisper", nil"#] {
            let err = s
                .run(&format!(r#"SendAddonMessage("p", "m", {args})"#))
                .expect_err(args);
            assert!(
                err.to_string()
                    .contains("SendAddonMessage(): Whisper message missing target player!"),
                "{args}: {err}"
            );
        }
        assert!(s.take_addon_sends().is_empty());
        // Another lane ignores the fourth argument.
        s.run(r#"SendAddonMessage("p", "m", "GUILD", "Bob")"#)
            .unwrap();
        assert_eq!(s.take_addon_sends()[0].target, None);
    }

    #[test]
    fn a_number_type_raises_rather_than_being_read_as_a_type_byte() {
        let mut s = UiScript::new().unwrap();
        let err = s
            .run(r#"SendAddonMessage("p", "m", 1)"#)
            .expect_err("a numeric type must raise");
        assert!(err.to_string().contains("Unknown addon chat type"), "{err}");
        assert!(s.take_addon_sends().is_empty());
    }

    #[test]
    fn a_non_string_type_takes_the_party_default() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("p", "m", nil)"#).unwrap();
        s.run(r#"SendAddonMessage("p", "m", {})"#).unwrap();
        let sent = s.take_addon_sends();
        assert_eq!(sent.len(), 2);
        assert!(sent
            .iter()
            .all(|a| a.distribution == AddonDistribution::Party));
    }

    #[test]
    fn only_both_empty_is_a_usage_error() {
        let mut s = UiScript::new().unwrap();
        let err = s
            .run(r#"SendAddonMessage("", "")"#)
            .expect_err("both empty must raise");
        assert!(
            err.to_string()
                .contains(r#"Usage: SendAddonMessage("prefix", "message" [,"type"] [,"target"])"#),
            "{err}"
        );
        assert!(s.take_addon_sends().is_empty());
        s.run(r#"SendAddonMessage("LOOT_OPENED", "", "RAID")"#)
            .unwrap();
        s.run(r#"SendAddonMessage("", "m")"#).unwrap();
        let sent = s.take_addon_sends();
        assert_eq!(sent[0].text, b"LOOT_OPENED\t");
        assert_eq!(sent[1].text, b"\tm");
    }

    #[test]
    fn arguments_coerce_like_lua_tostring_and_nil_becomes_empty() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("v", 314)"#).unwrap();
        assert_eq!(sent_text(&mut s), b"v\t314");
        s.run(r#"SendAddonMessage(42, "m")"#).unwrap();
        assert_eq!(sent_text(&mut s), b"42\tm");
        let err = s
            .run(r#"SendAddonMessage({}, {})"#)
            .expect_err("two tables coerce to two empty strings");
        assert!(err.to_string().contains("Usage: SendAddonMessage"), "{err}");
    }

    /// The verb runs no escape check (`0x500560` has none): pipes go out as written.
    #[test]
    fn the_text_goes_out_verbatim_with_no_escape_check() {
        let mut s = UiScript::new().unwrap();
        s.run("SendAddonMessage(\"pre\\tfix\", \"|z ||| |cff00 |\", \"GUILD\")")
            .unwrap();
        assert_eq!(sent_text(&mut s), b"pre\tfix\t|z ||| |cff00 |");
    }

    /// The compose writes at most 2999 bytes (`0x5005ea`, `0x5005f0`), cutting inside a UTF-8
    /// sequence when the line ends there.
    #[test]
    fn the_payload_cuts_at_2999_bytes_even_inside_a_character() {
        let mut s = UiScript::new().unwrap();
        s.run(r#"SendAddonMessage("p", string.rep("x", 4000))"#)
            .unwrap();
        let text = sent_text(&mut s);
        assert_eq!(text.len(), 2999);
        assert!(text.starts_with(b"p\tx"));
        s.run(r#"SendAddonMessage("p", string.rep("y", 2997))"#)
            .unwrap();
        assert_eq!(sent_text(&mut s).len(), 2999);
        // 2996 bytes then a two-byte character: the cut keeps its lead byte alone.
        s.run(r#"SendAddonMessage("p", string.rep("z", 2996) .. "\195\169")"#)
            .unwrap();
        let text = sent_text(&mut s);
        assert_eq!(text.len(), 2999);
        assert_eq!(text[2998], 0xC3);
    }

    #[test]
    fn the_tokens_round_trip() {
        for (dist, token) in [
            (AddonDistribution::Party, "PARTY"),
            (AddonDistribution::Raid, "RAID"),
            (AddonDistribution::Guild, "GUILD"),
            (AddonDistribution::Battleground, "BATTLEGROUND"),
            (AddonDistribution::Whisper, "WHISPER"),
        ] {
            assert_eq!(dist.token(), token);
            assert_eq!(AddonDistribution::from_token(token), Some(dist));
        }
    }
}
