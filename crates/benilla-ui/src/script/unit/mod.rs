//! The `Unit*` bindings. The app pushes each token's [`UnitState`] every frame through
//! [`UiScript::set_unit`], and the units a `target` chain ends on by guid through
//! [`UiScript::set_unit_by_guid`]; the globals read that plain data, which keeps this crate free of
//! the ECS. Every predicate answers the number `1` or nil, never a Lua boolean.

use mlua::Lua;

use super::{NearestMode, ScriptCall};

/// A selection ask from Lua, queued in call order for the app to resolve and commit. The reference
/// routes `TargetUnit`, `AssistUnit` and `TargetLastEnemy` through one helper (else the group
/// roster, else a silent return), and a macro can observe their order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionRequest {
    /// `TargetUnit(unit)`: the unit the token names.
    Unit(String),
    /// `AssistUnit(unit)`: the token's own `UNIT_FIELD_TARGET`; a token with no target is a silent
    /// no-op that sends nothing.
    Assist(String),
    /// `AssistByName(name)`, the `/assist <name>` verb: what the named player targets, resolved by
    /// name rather than by token.
    AssistByName(String),
    /// `TargetLastEnemy()`: the last attackable unit committed.
    LastEnemy,
}

/// The reference's local player record: a copy of the chosen `SMSG_CHAR_ENUM` row, written once at
/// the Enter World commit before FrameXML loads, and never cleared. `UnitName`, `UnitRace`,
/// `UnitClass` and `UnitSex` answer `"player"`, matched whole in any case, from it and never from
/// the descriptor, so they answer before the player object exists. `UnitLevel` reads the
/// descriptor: the record's level accessor has no caller. Race and class are held resolved, as this
/// crate has no DBC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerRecord {
    /// `+0x08`, the name; empty is the unset record, whose `UnitName("player")` is nil.
    pub name: String,
    /// `+0x100` as `UnitRace`'s (localized, file token) pair; `None` is race 0, which has no
    /// `ChrRaces` row and answers `nil, nil`.
    pub race: Option<(String, String)>,
    /// `+0x101` as `UnitClass`'s (localized, uppercase token) pair; `None` answers `nil, nil`.
    pub class: Option<(String, String)>,
    /// `+0x102` on `UnitSex`'s scale (2 male, 3 female), not the wire's 0/1. The unset 0 answers 2,
    /// as the reference's all-zero record does: `UnitSex` indexes `{2,3,1,6}` with no bounds check.
    pub sex: u8,
}

/// One unit's snapshot, pushed by the app each frame under a token, or under a guid for a unit a
/// `target` chain ends on, and read by the `Unit*` bindings; plain data, with no mlua handles or
/// ECS types.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitState {
    /// `UnitExists`, true for an out-of-range party member through the roster fallback. The
    /// reference also requires a held object to be selectable before that fallback (:
    /// `UNIT_FIELD_FLAGS & 0x02000000` clear, or created by us); that is not built.
    pub exists: bool,
    /// `UnitIsVisible`: the client holds an object for this token, and nothing more; the range test
    /// is the server's, which drops out-of-range objects. Not [`Self::exists`]: an out-of-range
    /// party member exists without an object.
    pub has_object: bool,
    /// `UnitIsTapped`: `UNIT_DYNAMIC_FLAGS` (field 143) bit `0x4`, someone holds the kill credit.
    /// Set only from a live descriptor, which supplies the reference's object-presence check.
    pub tapped: bool,
    /// `UnitIsPartyLeader`'s first leg: `PLAYER_FLAGS & 0x1` on a player's own descriptor, true for
    /// a stranger leading their own party. The reference ORs it with a compare against our group
    /// leader's guid.
    pub group_leader: bool,
    /// `UnitIsTappedByPlayer`: the same field's bit `0x8`. Tapped without it is someone else's
    /// kill, the grey unit frame.
    pub tapped_by_player: bool,
    /// `UnitName`'s name; `None` until the name query answers, which reads as [`unknownobject`].
    pub name: Option<String>,
    /// Current health (`UnitHealth`).
    pub health: u32,
    /// Maximum health (`UnitHealthMax`).
    pub max_health: u32,
    /// Level (`UnitLevel`).
    pub level: u32,
    /// The active power type (`UnitPowerType`), `UNIT_FIELD_BYTES_0` byte 3: 0 mana, 1 rage,
    /// 2 focus, 3 energy, 4 happiness, 5 runes, 6 runic power.
    pub power_type: u8,
    /// Current power of the active type, shown scale (`UnitPower` with no type).
    pub power: u32,
    /// Maximum power of the active type, shown scale (`UnitPowerMax` with no type).
    pub max_power: u32,
    /// Current power of each type 0..=6 (mana, rage, focus, energy, happiness, runes, runic
    /// power), shown scale, which `UnitPower(unit, type)` reads for the local player.
    pub shown_powers: [u32; 7],
    /// Maximum power of each type 0..=6, shown scale, which `UnitPowerMax(unit, type)` reads for
    /// any held unit.
    pub shown_max_powers: [u32; 7],
    /// `UnitIsDead`; a released ghost is not dead by it, as its health is 1.
    pub dead: bool,
    /// `UnitIsCharmed`: `UNIT_FIELD_CHARMEDBY` (fields 10-11) is non-zero. `UNIT_FIELD_CHARM` is
    /// never read, so a charmer answers nil, as does a summoned pet.
    pub charmed: bool,
    /// `UnitIsGhost`: `PLAYER_FLAGS` bit `0x10`, so players only.
    pub ghost: bool,
    /// The reaction toward the player on `UnitReaction`'s scale, 1 hated to 7 revered, which
    /// Exalted reads too, 0 unknown (nil). Fed for the tokens naming another unit; `"player"` and
    /// `"pet"` stay 0.
    pub reaction: u8,
    /// The player's reaction toward the unit, `UnitReaction("player", unit)` on the same scale:
    /// 12340's `0x7251c0` with the player as `this`, which answers a reputation faction by the
    /// at-war bit alone, so it can differ from [`Self::reaction`]. Fed where that is; 0 unknown.
    pub reaction_from_player: u8,
    /// `UnitSelectionColor` (`0x60da00`): the ARGB dword `0x521bf0` resolves for the unit, which
    /// the binding splits into r, g, b and a over 255. 0 is unfed, read as the resolver's answer
    /// for a unit it holds no object for, blue.
    pub selection_color: u32,
    /// `UnitRace`'s localized first return, from `UNIT_FIELD_BYTES_0` byte 0; `None` answers
    /// `nil, nil`.
    pub race: Option<String>,
    /// `UnitRace`'s second return, the file token (`"NightElf"`) texture paths are built from.
    pub race_file: Option<String>,
    /// `UnitClass`'s localized first return.
    pub class: Option<String>,
    /// `UnitClass`'s second return, the uppercase token (`"WARRIOR"`).
    pub class_file: Option<String>,
    /// `UnitHasRelicSlot`: INVSLOT 17 is a relic slot, from `ChrClasses.dbc` field
    /// 16. Players only, true for Paladin, Shaman and Druid.
    pub has_relic_slot: bool,
    /// `ChrClasses.dbc` field 2 for a player's class, 0-based (Strength 0, Agility 1), which
    /// `GetDamageBonusStat` reads for the active player; `None` with no class row.
    pub damage_bonus_stat: Option<u32>,
    /// `UnitSex`'s scale, 2 male and 3 female, from `UNIT_FIELD_BYTES_0` byte 2 (0 male, 1 female).
    /// The unfilled 0 answers 2, the reference's unresolved answer.
    pub sex: u8,
    /// `UnitIsPlayer`: a player character by guid, whose tooltip level line shows race and class.
    pub is_player: bool,
    /// `UnitPlayerControlled`: `UNIT_FIELD_FLAGS` bit `0x8`, which a player's pet and a charmed
    /// creature carry as well as a player.
    pub player_controlled: bool,
    /// `UNIT_FIELD_FLAGS`, raw: `UnitIsPlusMob` reads bit `0x40`, and `UNIT_FLAGS` fires when it
    /// moves.
    pub flags: u32,
    /// `PLAYER_FLAGS` (field 190), raw, 0 on a creature. `PLAYER_FLAGS_CHANGED` fires when any bit
    /// moves on any player, remote ones included: the reference's handler fires before any bit test
    /// or local-player check. Raw, as most of its bits are not decoded here.
    pub player_flags: u32,
    /// `UnitIsAFK` (`0x60cc30`): a held player's `PLAYER_FLAGS` `0x2`, else the party record's
    /// status `0x40`.
    pub afk: bool,
    /// `UnitIsDND` (`0x60cd50`): a held player's `PLAYER_FLAGS` `0x4`, else the party record's
    /// status `0x80`.
    pub dnd: bool,
    /// `UnitGroupRolesAssigned` (`0x60c810`): the LFG role bits `SMSG_GROUP_LIST` carries for a
    /// group member or the player, tank `0x2`, healer `0x4`, damage `0x8`; 0 outside the group.
    pub group_roles: u8,
    /// `UnitInVehicle` (`0x6133d0`)'s unheld leg: the party record's vehicle status bit `0x200`;
    /// a held unit's passenger state rides [`super::VehicleView`].
    pub in_vehicle: bool,
    /// `UNIT_DYNAMIC_FLAGS` (field 143), raw. The event of the same name fires when any bit moves:
    /// the reference's field watch compares the whole dword, and bits such as `0x2` (tracked) are
    /// not decoded here.
    pub dynamic_flags: u32,
    /// The owner: `UNIT_FIELD_SUMMONEDBY`, else the charmer, else the creator, else 0; the
    /// "or pet" half of `UnitPlayerOrPetInParty` and `UnitPlayerOrPetInRaid`.
    pub owner: u64,
    /// `GetGuildInfo(unit)`, from descriptor fields 191-192 and the guild-name cache; `None`
    /// when guildless or not yet streamed.
    pub guild: Option<super::guild::UnitGuild>,
    /// The creature template's subname ("Stable Master"), the tooltip's second line.
    pub subtitle: Option<String>,
    /// The word ("Beast") from `CreatureType.dbc` for the type the reference's resolver returns for
    /// any unit: the shapeshift form's, else the creature template's, else the race's.
    /// `UnitCreatureType` answers it, and a creature's level line takes it as its class slot; a
    /// player's reads race and class instead. `None` for type 0 or no descriptor.
    pub creature_type_name: Option<String>,
    /// Creature rank 0..=4 as the reference's getter answers it, so already gated: 0 without a
    /// cached creature template or with a pet number. The tooltip's rank word, `UnitLevel`'s boss
    /// -1 and `UnitClassification` all read it.
    pub rank: u32,
    /// The creature civilian flag, one term of [`is_civilian_kill`].
    pub civilian: bool,
    /// The creature racial-leader flag, the tooltip's LEADER line when PvP-flagged.
    pub racial_leader: bool,
    /// `UNIT_FIELD_FLAGS`' PvP bit: `UnitIsPVP` and the tooltip's PvP line.
    pub pvp: bool,
    /// The GlobalString of the tooltip's red skinnable line (`UNIT_SKINNABLE_LEATHER`, `_HERB`,
    /// `_ROCK`, `_BOLTS`), set for a non-player with `UNIT_FIELD_FLAGS`' skinnable bit.
    pub skinnable: Option<&'static str>,
    /// A dead unit's master looter and round-robin looter by name (`SMSG_LOOT_LIST`), the
    /// tooltip's `MASTER_LOOTER` and `LOOT` lines; `None` while unnamed or unset.
    pub master_looter: Option<String>,
    pub looter: Option<String>,
    /// The faction name ("Stormwind"), the tooltip line after the level line, from `Faction.dbc`
    /// through the faction template with its hiding gates applied; `None` shows no line.
    pub faction_name: Option<String>,
    /// `UnitIsConnected`: any unit the object manager holds answers 1; a party token with no object
    /// takes the roster status byte's `0x01`. The default `false` reads disconnected, which greys a
    /// stock mana bar (`UnitFrame.lua:214`), so a hand-built live unit must set it; the app's
    /// `snapshot` does for every held one.
    pub is_connected: bool,
    /// `UnitIsPVPFreeForAll`: `PLAYER_FLAGS` bit `0x80`, or the roster status byte's `0x10`;
    /// independent of [`Self::pvp`].
    pub is_pvp_ffa: bool,
    /// `UnitPVPRank`: the current rank on the internal `0..=18` scale (0 none), from the public
    /// `PLAYER_BYTES_3` byte 3. The private lifetime highest rides
    /// [`HonorState`](super::HonorState); [`super::pvp`] converts to the `-4..=14` visual scale.
    pub pvp_rank: u8,
    /// The city-protector title, `PLAYER_BYTES_3` byte 2 (0 none): `UnitPVPName` appends the
    /// `PVP_MEDAL<n>` line under a chosen title for a non-zero one (`0x72a35d`).
    pub pvp_medal: u8,
    /// A player's `PLAYER_CHOSEN_TITLE`, a `CharTitles.dbc` mask id, 0 for none.
    pub chosen_title: i32,
    /// `UnitFactionGroup`'s first return, `"Alliance"` or `"Horde"`: `FactionGroup.dbc`'s English
    /// `InternalName` for the side bit of the faction template's group mask, which stock builds
    /// texture paths from (`PlayerFrame.lua:68`). `None` for a template with no side.
    pub faction_group: Option<String>,
    /// `UnitFactionGroup`'s second return, `FactionGroup.dbc`'s localized `Name0`: display text.
    pub faction_group_localized: Option<String>,
    /// The team digit of `PVP_RANK_<rank>_<team>`: 0 Horde, 1 Alliance, -1 no side. It follows the
    /// race through `ChrRaces` and `FactionTemplate`, never the live template `UnitFactionGroup`
    /// reads for any token but `"player"` (`0x60d167`), so a GM keeps it. The default 0 is the
    /// reference's answer for a unit it cannot resolve.
    pub pvp_team: i8,
    /// `OBJECT_FIELD_GUID`, which `UnitIsUnit` and `UnitInParty` compare; 0 is unresolved, and
    /// two zeros never match.
    pub guid: u64,
    /// A creature or vehicle whose template has type flag `0x4000`: `UnitGUID` (`0x60e630`)
    /// shows its guid with the spawn counter, the low 24 bits, zeroed.
    pub counter_hidden: bool,
    /// `GetRaidTargetIndex`: 0 none, else the mark 1..=8, from the group's raid-target board.
    pub raid_target: u8,
    /// Whether the player can attack the unit, `UnitCanAttack("player", unit)`, which the binding
    /// (delegating to the attack check) answers for both argument orders. Fed for `target`,
    /// `targettarget`, `npc` and the units a chain ends on; other tokens read false.
    pub can_attack: bool,
    /// Whether the player can assist the unit, `UnitCanAssist("player", unit)` (12340's assist
    /// check, `0x7293d0`); fed where [`Self::can_attack`] is, and true for the player.
    pub can_assist: bool,
    /// `UnitIsCorpse`: the token names a `TYPEID_CORPSE` object, which a dead unit is not. No feed
    /// sets it.
    pub corpse_object: bool,
    /// `UnitAffectingCombat`: `UNIT_FIELD_FLAGS` bit `0x00080000`, for every token including
    /// `"player"`; the reference has no separate player combat flag.
    pub in_combat: bool,
}

/// The name of a unit whose name is not yet known, as the reference's `UnitName` and unit tooltip
/// title both answer it: the `UNKNOWNOBJECT` GlobalString (enUS "Unknown"), else the literal
/// "Unknown Being" when the global is missing or empty.
pub fn unknownobject(lua: &Lua) -> mlua::Result<mlua::String> {
    // One expression: `benilla-app`'s `reference_strings` check matches
    // `globals().get::<String>`, and a line break inside it would hide this lookup.
    let global = lua.globals().get::<String>("UNKNOWNOBJECT").ok();
    lua.create_string(match global.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => "Unknown Being",
    })
}

/// The reference's grey-level band, indexed by player level / 5.
const GREY_BAND: [u32; 20] = [
    4, 4, 5, 5, 6, 6, 7, 7, 8, 9, 10, 11, 12, 12, 12, 12, 12, 12, 12, 12,
];

/// `GetQuestGreenRange`'s answer: the band for a player level, 12 past the table.
pub fn grey_band(player_level: u32) -> u32 {
    GREY_BAND
        .get((player_level / 5) as usize)
        .copied()
        .unwrap_or(12)
}

/// The reference's grey level test: the unit is more than the band below the player, strictly, so a
/// gap equal to the band is still green. The reference also returns false for a player (object type
/// bit `0x10`); a pet takes the level test.
pub fn unit_is_grey(player_level: u32, unit_level: u32) -> bool {
    player_level > unit_level && player_level - unit_level > grey_band(player_level)
}

/// The civilian gate, a dishonorable kill: PvP-flagged, a civilian creature, hostile
/// (`UnitReaction` <= 2) and grey. The tooltip's CIVILIAN line and `UnitPVPName`'s civilian prefix
/// both read it. The terms are the tooltip line's; the reference gate's body is not traced.
pub fn is_civilian_kill(u: &UnitState, player_level: u32) -> bool {
    u.civilian && u.pvp && u.reaction != 0 && u.reaction <= 2 && unit_is_grey(player_level, u.level)
}

/// The tooltip's "??" level gate: never a player; level 0, world-boss rank 3, or a hostile
/// (reaction <= 2) at least 10 levels above the player. `UnitLevel` answers the boss and hostile
/// legs with -1, but returns a level of 0 as is.
pub fn level_reads_unknown(u: &UnitState, player_level: u32) -> bool {
    let much_higher_hostile =
        u.reaction != 0 && u.reaction <= 2 && u.level >= player_level.saturating_add(10);
    !u.is_player && (u.level == 0 || u.rank == 3 || much_higher_hostile)
}

/// `UnitClassification`: the word table indexed by the gated [`UnitState::rank`]; an unresolved
/// token reads index 0, "normal". "rareelite" is a real answer, though stock draws it with the
/// elite art (`TargetFrame.lua:209`).
pub fn classification_word(rank: u32) -> &'static str {
    match rank {
        1 => "elite",
        2 => "rareelite",
        3 => "worldboss",
        4 => "rare",
        _ => "normal",
    }
}

/// The resource name for a power type, the suffix of its per-resource events (`UNIT_RAGE`,
/// `UNIT_MAXRAGE`: ids 19 and 27 plus the type); runes (5) have none, as ids 24 and 32 are
/// unnamed, and an unknown index reads as mana, the descriptor default.
pub fn power_token(ty: u8) -> Option<&'static str> {
    match ty {
        1 => Some("RAGE"),
        2 => Some("FOCUS"),
        3 => Some("ENERGY"),
        4 => Some("HAPPINESS"),
        5 => None,
        6 => Some("RUNIC_POWER"),
        _ => Some("MANA"),
    }
}

impl super::UiScript {
    /// Push, or clear with `None`, a token's snapshot; the app's feed calls it each frame before
    /// event dispatch, so a frame's `OnEvent` sees current values.
    pub fn set_unit(&mut self, token: &str, state: Option<UnitState>) {
        {
            let mut model = self.model_mut();
            match state {
                // Keys fold to lowercase on the way in too, so a `"Target"` push cannot shadow
                // `"target"`.
                Some(s) => {
                    // A `"player"` push updates each record field it carries and never blanks
                    // one, so a nameless snapshot cannot reach the four record verbs.
                    if token.eq_ignore_ascii_case("player") {
                        let rec = &mut model.player_record;
                        if let Some(name) = s.name.as_deref().filter(|n| !n.is_empty()) {
                            if rec.name != name {
                                rec.name = name.to_string();
                            }
                        }
                        if let Some(race) = s.race.clone().zip(s.race_file.clone()) {
                            rec.race = Some(race);
                        }
                        if let Some(class) = s.class.clone().zip(s.class_file.clone()) {
                            rec.class = Some(class);
                        }
                        if s.sex != 0 {
                            rec.sex = s.sex;
                        }
                    }
                    model.units_by_lower.insert(token.to_ascii_lowercase(), s);
                }
                None => {
                    model.units_by_lower.remove(&token.to_ascii_lowercase());
                }
            }
        }
        // A push of the unit a tooltip shows re-drives its health bar, without a line rebuild.
        super::tooltip_unit::on_unit_push(&self.lua, token);
    }

    /// Push, or clear with `None`, the snapshot of a unit a `target` chain can end on, by guid. A
    /// token with a hop that no push names (`"party1target"`, `"raid3targettarget"`) resolves
    /// through [`UnitGuids`] to a guid and reads that guid's entry, the reference's order (then
    /// each getter's object lookup); the guids to cover are [`Self::chain_end_guids`]. A token
    /// without a hop reads its own push, not this.
    pub fn set_unit_by_guid(&mut self, guid: u64, state: Option<UnitState>) {
        let mut model = self.model_mut();
        match state {
            Some(s) => {
                model.units_by_guid.insert(guid, s);
            }
            None => {
                model.units_by_guid.remove(&guid);
            }
        }
    }

    /// The guids the chains a script can spell end on ([`UnitGuids::chain_ends`]), each once: the
    /// units whose snapshots [`Self::set_unit_by_guid`] serves.
    pub fn chain_end_guids(&self) -> Vec<u64> {
        let mut ends: Vec<u64> = self.model_ref().unit_guids.chain_ends().collect();
        ends.sort_unstable();
        ends.dedup();
        ends
    }

    /// Push the player's copper (`PLAYER_FIELD_COINAGE`), read by `GetMoney`.
    pub fn set_money(&mut self, copper: u64) {
        set_field!(self, money, copper);
    }

    /// Push `PLAYER_XP` and `PLAYER_NEXT_LEVEL_XP`, private fields that `UnitXP` and `UnitXPMax`
    /// answer for `"player"` alone.
    pub fn set_player_xp(&mut self, xp: u32, next_level_xp: u32) {
        let mut model = self.model_mut();
        model.player_xp = xp;
        model.player_next_level_xp = next_level_xp;
    }

    /// Push the rest state (`PLAYER_BYTES_2` byte 3), the rested pool
    /// (`PLAYER_REST_STATE_EXPERIENCE`, raw) and `PLAYER_FLAGS_RESTING` together, so
    /// `GetRestState`, `GetXPExhaustion` and `IsResting` never read a half update.
    pub fn set_rest_state(&mut self, state: u8, pool: u32, resting: bool) {
        let mut model = self.model_mut();
        model.rest_state = state;
        model.rest_pool = pool;
        model.resting = resting;
    }

    /// Push `PLAYER_FLAGS` bits 12 (`PartialPlayTime`) and 13 (`NoPlayTime`) together, as
    /// `PlayerFrame.lua:244` tests them in one if/elseif.
    pub fn set_play_time(&mut self, partial: bool, none: bool) {
        let mut model = self.model_mut();
        model.partial_play_time = partial;
        model.no_play_time = none;
    }

    /// Push `PLAYER_FLAGS` bit 25 (`0x2000000`), the no-XP flag `IsXPUserDisabled()` reads.
    pub fn set_xp_user_disabled(&mut self, disabled: bool) {
        set_field!(self, xp_user_disabled, disabled);
    }

    /// Push the account's expansion byte, before the load, as stock files read it at load time.
    pub fn set_account_expansion(&mut self, expansion: u8) {
        set_field!(self, account_expansion, expansion);
    }

    /// Push `GetBillingTimeRested()`'s minutes, from the `SMSG_AUTH_RESPONSE` that admitted the
    /// session; nothing else on the wire writes it.
    pub fn set_billing_time_rested(&mut self, minutes: u32) {
        set_field!(self, billing_time_rested, minutes);
    }

    /// Enter the UI-load sound-suppression scope: a depth every name-keyed `PlaySound` reads, so
    /// scopes nest and only the outermost exit re-enables sound.
    pub fn push_sound_suppression(&self) {
        let mut model = self.model_mut();
        model.sound_suppression = model.sound_suppression.saturating_add(1);
    }

    /// Leave it, saturating so an unbalanced pop cannot wrap to a muted UI.
    pub fn pop_sound_suppression(&self) {
        let mut model = self.model_mut();
        model.sound_suppression = model.sound_suppression.saturating_sub(1);
    }

    /// Push whether a cinematic is playing, `InCinematic()`'s answer.
    pub fn set_in_cinematic(&mut self, playing: bool) {
        set_field!(self, in_cinematic, playing);
    }

    /// Push `Exhaustion.dbc`'s (rest state, localized name, factor) rows, read as `0x51b8b0` and
    /// `0x51b960` read them; an empty push keeps the fallback table the model seeds.
    pub fn set_exhaustion_rows(&mut self, rows: Vec<(u8, String, f64)>) {
        if rows.is_empty() {
            return;
        }
        let mut model = self.model_mut();
        model.exhaustion = rows.into_iter().map(|(id, n, f)| (id, (n, f))).collect();
    }

    /// Push our combo points and the unit they sit on, as the game UI's combo-point update banks
    /// them. Raw: `GetComboPoints`' class gate is the binding's.
    pub fn set_combo_points(&mut self, points: u8, target: u64) {
        let mut model = self.model_mut();
        model.combo_points = points;
        model.combo_target = target;
    }

    /// Push the pet's combo points and their unit (the pet state's combo-point set, `0x5d34b0`).
    pub fn set_pet_combo_points(&mut self, points: u8, target: u64) {
        let mut model = self.model_mut();
        model.pet_combo_points = points;
        model.pet_combo_target = target;
    }

    /// Take the queued [`SelectionRequest`]s out of the call stream, in call order.
    pub fn take_selection_requests(&mut self) -> Vec<SelectionRequest> {
        self.take_calls_where(|c| match c {
            ScriptCall::Select(r) => Some(r.clone()),
            _ => None,
        })
    }

    /// Take the `TargetNearest*([reverse])` calls out of the call stream, `true` for reverse. They
    /// name no unit: the reference runs the TAB cycler straight into `SetSelection`.
    pub fn take_target_nearest_requests(&mut self) -> Vec<(NearestMode, bool)> {
        self.take_calls_where(|c| match c {
            ScriptCall::TargetNearest { mode, reverse } => Some((*mode, *reverse)),
            _ => None,
        })
    }

    /// Take the `TargetByName(name, exactMatch)` calls out of the call stream.
    pub fn take_target_by_name_requests(&mut self) -> Vec<(String, bool)> {
        self.take_calls_where(|c| match c {
            ScriptCall::TargetByName { name, exact } => Some((name.clone(), *exact)),
            _ => None,
        })
    }

    /// Take the `ClearTarget()` calls out of the call stream: whether there was one.
    pub fn take_target_clear(&mut self) -> bool {
        !self
            .take_calls_where(|c| matches!(c, ScriptCall::ClearTarget).then_some(()))
            .is_empty()
    }

    /// Drain `DropItemOnUnit`'s tokens; on the pet the app casts the learned Feed Pet spell at the
    /// held item.
    pub fn take_drop_item_on_unit(&mut self) -> Vec<String> {
        drain_field!(self, drop_item_on_unit)
    }

    /// Take the `SpellTargetUnit` calls out of the call stream.
    pub fn take_spell_target_unit(&mut self) -> Vec<String> {
        self.take_calls_where(|c| match c {
            ScriptCall::SpellTargetUnit(token) => Some(token.clone()),
            _ => None,
        })
    }
}

/// The non-`"player"` token of a two-unit call such as `UnitIsEnemy(a, b)`, which the snapshot
/// holds the relationship on; the test folds case, as the resolver does.
fn pick_unit_token(a: &Option<String>, b: &Option<String>) -> Option<String> {
    match (a, b) {
        (Some(x), _) if !x.eq_ignore_ascii_case("player") => Some(x.clone()),
        (_, Some(y)) => Some(y.clone()),
        (x, _) => x.clone(),
    }
}

/// Map a token's snapshot through `f` under a short model borrow, or answer `default` for an
/// absent unit, after the token check.
fn with_unit<T>(
    lua: &Lua,
    token: &Option<String>,
    default: T,
    f: impl FnOnce(&UnitState) -> T,
) -> mlua::Result<T> {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    Ok(match token.as_ref().and_then(|t| model.unit(t)) {
        Some(u) => f(u),
        None => default,
    })
}

/// 12340's reaction routine `0x7251c0` with `a` as `this` toward `b`, on `UnitReaction`'s 1-based
/// scale, as `UnitReaction`, `UnitIsEnemy` and `UnitIsFriend` ask it (`0x60d2d7`, `0x60d386`,
/// `0x514050`); `None` when either token names no unit (`0x60d2ce`). A unit toward itself is 5
/// (`0x7251d5`). The snapshots hold the two directions between the player and a unit, so a pair
/// naming the player answers from the other unit's, where a snapshot with no player-side
/// direction fed (party, pet) answers its own; a pair without the player is not built and
/// answers `a`'s reaction toward the player. 0 is an unfed reaction and answers `None` too.
fn pair_reaction(lua: &Lua, a: &str, b: &str) -> Option<u8> {
    let model = lua
        .app_data_ref::<crate::script::ModelCell>()
        .expect("model app_data");
    let (ua, ub) = (model.unit(a)?, model.unit(b)?);
    let player = model.unit_guids.player;
    let is_player = |token: &str, u: &UnitState| {
        token.eq_ignore_ascii_case("player") || (player != 0 && u.guid == player)
    };
    let (a_player, b_player) = (is_player(a, ua), is_player(b, ub));
    let r = if (a_player && b_player) || (ua.guid != 0 && ua.guid == ub.guid) {
        5
    } else if b_player {
        ua.reaction
    } else if a_player {
        match ub.reaction_from_player {
            0 => ub.reaction,
            r => r,
        }
    } else {
        ua.reaction
    };
    (r != 0).then_some(r)
}

/// A unit predicate's body: the number 1 or nil through [`super::binding_abi::flag`], never a Lua
/// boolean, as the reference pushes only `lua_pushnumber` or `lua_pushnil`. `UnitReaction`,
/// `UnitLevel` and `UnitPowerType` are getters with their own shapes and do not route here.
fn unit_predicate(
    lua: &Lua,
    token: &Option<String>,
    f: impl FnOnce(&UnitState) -> bool,
) -> mlua::Result<mlua::Value> {
    Ok(super::binding_abi::flag(with_unit(lua, token, false, f)?))
}

/// The `Unit*` and `GetQuestGreenRange` registrations.
mod bindings;
#[cfg(test)]
mod chain_tests;
mod resolve;
#[cfg(test)]
mod tests;

pub use resolve::{parse_unit_token, UnitBase, UnitGuids, UnitTokenParse};

pub(super) use bindings::install;
