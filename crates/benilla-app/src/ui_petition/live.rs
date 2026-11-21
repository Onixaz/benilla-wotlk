//! The charter windows as their verbs read and act on them at the call, and the walks the verbs and
//! the record callbacks share.

use benilla_protocol::messages::{ArenaBanner, ITEM_FLAG_CHARTER};
use benilla_ui::script::{
    petition_type_name, ArenaBannerChoice, PetitionArg, PetitionOutcome, PetitionRecordView,
    PetitionRequest, PetitionState as PetitionRead, PetitionStep, PetitionVendorItem,
};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::entities::ItemDisplays;
use crate::items::Items;
use crate::names::NameCache;
use crate::net::{ClientCommand, NetCommands, ObjectStore, Objects, SelfGuid, SelfPlayer};
use crate::ui_action::UiError;

use super::{lines, GuildRegistrarState, PetitionState, PetitionVendorState, Wait};

/// A composed line as a step shown inside the call.
pub(super) fn step(line: UiError) -> PetitionStep {
    use crate::ui_action::FillArg;
    let arg = match line.args.into_iter().next() {
        Some(FillArg::S(s)) => PetitionArg::S(s),
        Some(FillArg::D(d)) => PetitionArg::D(d),
        _ => PetitionArg::None,
    };
    PetitionStep::Line(line.key, arg)
}

/// What the charter walks read: the player, the bags' items and their templates, and the wire.
pub(super) struct Walk<'a, 'w, 's> {
    pub(super) me: &'a ObjectStore,
    pub(super) objects: &'a Objects<'w, 's>,
    pub(super) items: &'a Items,
    pub(super) commands: &'a NetCommands,
}

impl Walk<'_, '_, '_> {
    /// The carried items in the charter walks' order: the backpack's sixteen (`0x6dd3ae`, `0x6dd6d2`), then each bag's contents.
    fn carried(&self) -> Vec<u64> {
        let mut out = Vec::new();
        for bag in 0..=4 {
            for slot0 in 0..36u8 {
                if bag == 0 && slot0 >= 16 {
                    break;
                }
                if let Some(guid) = crate::ui_items::slot_guid(&self.me.0, bag, slot0, self.objects)
                {
                    out.push(guid);
                }
            }
        }
        out
    }

    /// A carried charter's petition id: an item whose cached template has the petition flag, read off its first enchantment.
    fn charter(&self, guid: u64) -> Option<u32> {
        let f = self.objects.object(guid)?;
        let charter = f
            .object_entry()
            .and_then(|e| self.items.template(e, guid, self.commands))
            .is_some_and(|t| t.flags & ITEM_FLAG_CHARTER != 0);
        charter.then(|| f.item_enchant(0).unwrap_or(0) as u32)
    }

    /// `TurnInGuildCharter` (`0x6dd3a0`): the first charter whose record is uncached asks it with the turn-in's callback and stops; the first guild one is turned in; none shows `ERR_NO_GUILD_CHARTER`.
    pub(super) fn turn_in_guild(&self, petition: &mut PetitionState) -> Vec<PetitionStep> {
        for guid in self.carried() {
            let Some(id) = self.charter(guid) else {
                continue;
            };
            let Some(kind) = petition.record_or_wait(id, guid, Wait::GuildTurnIn, self.commands)
            else {
                return Vec::new();
            };
            if kind.static_type == 0 {
                let _ = self
                    .commands
                    .0
                    .send(ClientCommand::TurnInPetition { item: guid });
                return Vec::new();
            }
        }
        vec![step(lines::no_charter_line())]
    }

    /// The arena charter search and turn-in (`0x6dd680`): every charter's record asked with the arena callback; the first arena one (of the vendor's team size on a turn-in) is found, and a turn-in sends it with the vendor's banner or shows `ERR_NO_ARENA_CHARTER`.
    pub(super) fn turn_in_arena(
        &self,
        petition: &mut PetitionState,
        vendor: &mut PetitionVendorState,
        turn_in: bool,
    ) -> (bool, Vec<PetitionStep>) {
        vendor.turn_in = turn_in;
        let banner = vendor.banner;
        let mut found = None;
        for guid in self.carried() {
            let Some(id) = self.charter(guid) else {
                continue;
            };
            let hit = petition
                .record_or_wait(id, guid, Wait::ArenaTurnIn, self.commands)
                .is_some_and(|r| {
                    r.static_type == super::STATIC_TYPE_ARENA
                        && (!turn_in || r.min_signatures == banner.team_size.wrapping_sub(1) as i32)
                });
            if hit {
                found = Some(guid);
                break;
            }
        }
        if !turn_in {
            return (found.is_some(), Vec::new());
        }
        let Some(item) = found else {
            return (false, vec![step(lines::no_arena_charter_line())]);
        };
        let _ = self.commands.0.send(ClientCommand::TurnInArenaPetition {
            item,
            banner: ArenaBanner {
                background_color: banner.background_color,
                emblem_style: banner.emblem_style,
                emblem_color: banner.emblem_color,
                border_style: banner.border_style,
                border_color: banner.border_color,
            },
        });
        (true, Vec::new())
    }
}

/// The charter windows the verbs read and act on at their call.
#[derive(SystemParam)]
pub(crate) struct PetitionLive<'w, 's> {
    pub(super) state: ResMut<'w, PetitionState>,
    pub(super) registrar: ResMut<'w, GuildRegistrarState>,
    pub(super) vendor: ResMut<'w, PetitionVendorState>,
    names: Res<'w, NameCache>,
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    objects: Objects<'w, 's>,
    stores: Query<'w, 's, &'static ObjectStore>,
    me: Query<'w, 's, &'static ObjectStore, With<SelfPlayer>>,
    self_guid: Res<'w, SelfGuid>,
    commands: Res<'w, NetCommands>,
    arena_teams: Res<'w, crate::ui_arena_team::ArenaTeams>,
    factions: Option<Res<'w, crate::target::Factions>>,
}

/// What an act reads beside the charter windows: the current target, the designer and the interact target.
pub(crate) struct ActContext<'a> {
    pub(crate) target: Option<u64>,
    pub(crate) designer: Option<u64>,
    pub(crate) interact: Option<&'a mut crate::ui_session::InteractTarget>,
}

impl PetitionLive<'_, '_> {
    /// The records the bags' charters read, for the container slots.
    pub(crate) fn records(&self) -> &PetitionState {
        &self.state
    }

    fn me_guid(&self) -> u64 {
        self.self_guid.0.unwrap_or(0)
    }

    /// The charter window at the call: the held record (`0xc22b40`), the signer names, `CanSignPetition` (`0x5cf040`) and the registrar's cost (`0x599f40`).
    pub(crate) fn view(&self) -> PetitionRead {
        let s = &*self.state;
        let me = self.me_guid();
        let signers: Vec<Option<String>> = s
            .signers
            .iter()
            .map(|g| self.names.resolve(*g, &self.commands).map(str::to_string))
            .collect();
        let record = s.held_record().map(|r| PetitionRecordView {
            petition_type: petition_type_name(r.static_type).to_string(),
            title: r.name.clone(),
            body_text: r.body_text.clone(),
            max_signatures: r.max_signatures,
            originator: self
                .names
                .resolve(r.owner, &self.commands)
                .map(str::to_string),
            is_originator: r.owner == me,
            min_signatures: r.min_signatures,
        });
        let in_guild = self
            .me
            .iter()
            .next()
            .is_some_and(|m| m.0.player_guild_id() != 0);
        let refused = s.held_record().is_some_and(|r| {
            let full = r.static_type <= 1
                && ((r.static_type == 0 && in_guild) || s.signers.len() as i32 >= r.max_signatures);
            full || r.owner == me
        });
        PetitionRead {
            charter_cost: self.registrar.row.charter_cost,
            signers,
            record,
            can_sign: !refused && !s.signers.contains(&me),
        }
    }

    pub(crate) fn vendor_open(&self) -> bool {
        self.vendor.vendor.is_some()
    }

    pub(crate) fn vendor_count(&self) -> usize {
        self.vendor.rows.len()
    }

    /// `GetPetitionItemInfo`'s row (`0x5a1fb0`): resolved once the item cache has it, else asked with the vendor's record callback (`0x5a1f30`).
    pub(crate) fn item(
        &self,
        index: u32,
        waits: Option<&mut crate::items::RecordWaits>,
    ) -> Option<PetitionVendorItem> {
        let row = self.vendor.rows.get(index as usize)?;
        if row.charter_entry == 0 {
            return None;
        }
        let name = match self.items.template_cached(row.charter_entry) {
            Some(t) => Some(t.name.clone()),
            None => {
                let _ = self.items.template(row.charter_entry, 0, &self.commands);
                if let Some(w) = waits {
                    w.wait(
                        row.charter_entry,
                        crate::items::RecordWait::PetitionVendor,
                        false,
                    );
                }
                None
            }
        };
        Some(PetitionVendorItem {
            texture: name.as_ref().and_then(|_| {
                crate::ui_items::item_icon(self.icons.as_deref(), row.charter_display_id)
            }),
            name,
            cost: row.charter_cost,
        })
    }

    /// A charter verb inside its call.
    pub(crate) fn act(&mut self, act: PetitionRequest, cx: ActContext<'_>) -> PetitionOutcome {
        let mut out = PetitionOutcome::default();
        let me_guid = self.me_guid();
        let commands = &*self.commands;
        match act {
            // `0x59a050`: a name, a registrar and a player; a guild member and short coinage refused.
            PetitionRequest::Buy(name) => {
                let Some(npc) = self.registrar.registrar.filter(|_| !name.is_empty()) else {
                    return out;
                };
                let Some(me) = self.me.iter().next() else {
                    return out;
                };
                if me.0.player_guild_id() != 0 {
                    out.steps.push(step(UiError::key("ERR_ALREADY_IN_GUILD")));
                } else if me.0.player_money().unwrap_or(0) < self.registrar.row.charter_cost {
                    out.steps.push(step(UiError::key("ERR_NOT_ENOUGH_MONEY")));
                } else {
                    let index = self.registrar.row.index;
                    let _ = commands
                        .0
                        .send(ClientCommand::PetitionBuy { npc, name, index });
                }
            }
            PetitionRequest::VendorBuy { index, name } => {
                out.steps = self.vendor_buy(index, name);
            }
            PetitionRequest::TurnIn => {
                if let Some(me) = self.me.iter().next() {
                    let walk = Walk {
                        me,
                        objects: &self.objects,
                        items: &self.items,
                        commands,
                    };
                    out.steps = walk.turn_in_guild(&mut self.state);
                }
            }
            PetitionRequest::VendorTurnInArena(choice) => {
                if self.vendor.vendor.is_none() {
                    return out;
                }
                self.vendor.banner = choice;
                if let Some(me) = self.me.iter().next() {
                    let walk = Walk {
                        me,
                        objects: &self.objects,
                        items: &self.items,
                        commands,
                    };
                    out.steps = walk
                        .turn_in_arena(&mut self.state, &mut self.vendor, true)
                        .1;
                }
            }
            PetitionRequest::HasFilled => {
                if let Some(me) = self.me.iter().next() {
                    let walk = Walk {
                        me,
                        objects: &self.objects,
                        items: &self.items,
                        commands,
                    };
                    out.filled = walk
                        .turn_in_arena(&mut self.state, &mut self.vendor, false)
                        .0;
                }
            }
            // `0x5a2600`: the button's item with five all-ones banner words.
            PetitionRequest::VendorTurnIn { item } => {
                if self.vendor.vendor.is_some() {
                    let banner = ArenaBanner {
                        background_color: u32::MAX,
                        emblem_style: u32::MAX,
                        emblem_color: u32::MAX,
                        border_style: u32::MAX,
                        border_color: u32::MAX,
                    };
                    let _ = commands
                        .0
                        .send(ClientCommand::TurnInArenaPetition { item, banner });
                }
            }
            // `0x5a24f0`: the interact target cleared on the vendor, whose interaction close runs the vendor leg first when it held it (`0x513083`), then the zero and the event.
            PetitionRequest::CloseVendor => {
                let Some(vendor) = self.vendor.vendor.take() else {
                    return out;
                };
                if cx.interact.is_some_and(|t| t.clear_in_call(vendor)) {
                    out.steps.push(PetitionStep::Fire(
                        benilla_ui::script::PETITION_VENDOR_CLOSED_EVENT,
                    ));
                }
                out.steps.push(PetitionStep::Fire(
                    benilla_ui::script::PETITION_VENDOR_CLOSED_EVENT,
                ));
            }
            // `0x599f00`: the same through the registrar's leg (`0x512ff1`).
            PetitionRequest::CloseRegistrar => {
                out.steps = self.close_registrar(cx.interact);
            }
            // `0x59a1f0`: the designer's opener for the registrar's guid, then the registrar's close.
            PetitionRequest::TabardInfo => {
                if let Some(me) = self.me.iter().next() {
                    let npc = self.registrar.registrar.unwrap_or(0);
                    if let Some(line) =
                        tabard_vendor_activate(npc, cx.designer.unwrap_or(0), me, commands)
                    {
                        out.steps.push(step(line));
                    }
                }
                out.steps.extend(self.close_registrar(cx.interact));
            }
            // `0x5cf020` -> `0x5ceef0` with nothing: the open petition's close.
            PetitionRequest::ClosePetition => {
                if self.state.close_open(me_guid, commands) {
                    out.steps.push(PetitionStep::Fire("PETITION_CLOSED"));
                }
                self.state.store(None, 0, 0, commands);
            }
            // `0x5cf140`: sent with the open petition's guid, the signed latch set.
            PetitionRequest::Sign(byte) => {
                if let Some(item) = self.state.petition {
                    let _ = commands.0.send(ClientCommand::PetitionSign { item, byte });
                    self.state.signed = true;
                }
            }
            PetitionRequest::Offer => {
                out.steps = self.offer(cx.target);
            }
            // `0x5cf450`: nothing with no petition open.
            PetitionRequest::Rename(name) => {
                if let Some(item) = self.state.petition {
                    let _ = commands
                        .0
                        .send(ClientCommand::PetitionRename { item, name });
                }
            }
            PetitionRequest::RenameRefused(key) => {
                if self.state.petition.is_some() {
                    out.steps.push(step(lines::name_refused_line(key)));
                }
            }
            // The client's own name validator refused it; no packet is built.
            PetitionRequest::NameRefused(key) => {
                out.steps.push(step(lines::name_refused_line(key)));
            }
        }
        out
    }

    /// The registrar's close (`0x599e90`, `0x599f00`): the interact target cleared on it, whose interaction close runs the registrar leg first when it held it (`0x512ff1`), then the zero and `GUILD_REGISTRAR_CLOSED`.
    fn close_registrar(
        &mut self,
        interact: Option<&mut crate::ui_session::InteractTarget>,
    ) -> Vec<PetitionStep> {
        let Some(npc) = self.registrar.registrar.take() else {
            return Vec::new();
        };
        let mut steps = Vec::new();
        if interact.is_some_and(|t| t.clear_in_call(npc)) {
            steps.push(PetitionStep::Fire(
                benilla_ui::script::GUILD_REGISTRAR_CLOSED_EVENT,
            ));
        }
        steps.push(PetitionStep::Fire(
            benilla_ui::script::GUILD_REGISTRAR_CLOSED_EVENT,
        ));
        steps
    }

    /// `BuyPetition`'s action (`0x5a2350`): a row, a name, a vendor and a player; a guild charter for a guild member, an arena one for a size the player holds a team of, and short coinage refused; an arena one off level 60, 70 and 80 shows its level and buys anyway (`0x5a2431`).
    fn vendor_buy(&self, index: u32, name: String) -> Vec<PetitionStep> {
        let (Some(npc), Some(row)) = (self.vendor.vendor, self.vendor.rows.get(index as usize))
        else {
            return Vec::new();
        };
        if name.is_empty() {
            return Vec::new();
        }
        let Some(me) = self.me.iter().next() else {
            return Vec::new();
        };
        if row.charter_type == 0 && me.0.player_guild_id() != 0 {
            return vec![step(UiError::key("ERR_ALREADY_IN_GUILD"))];
        }
        if row.charter_type == 1
            && self
                .arena_teams
                .holds_team_of_size(row.required_signatures + 1)
        {
            return vec![step(UiError::key("ERR_ALREADY_IN_ARENA_TEAM"))];
        }
        let mut steps = Vec::new();
        if row.charter_type == 1 {
            let level = me.0.unit_level().unwrap_or(0);
            if !matches!(level, 60 | 70 | 80) {
                let needed = if (71..80).contains(&level) { 80 } else { 70 };
                steps.push(step(UiError::args(
                    "ERR_ARENA_TEAM_LEVEL_TOO_LOW_I",
                    vec![crate::ui_action::FillArg::D(needed)],
                )));
            }
        }
        if me.0.player_money().unwrap_or(0) < row.charter_cost {
            steps.push(step(UiError::key("ERR_NOT_ENOUGH_MONEY")));
            return steps;
        }
        let _ = self.commands.0.send(ClientCommand::PetitionBuy {
            npc,
            name,
            index: row.index,
        });
        steps
    }

    /// `OfferPetition` (`0x5cf220`): a petition, a locked target, a player and a held record, else silent; a target not held as a unit is out of range, one not a player silent; ourselves, a target we cannot cooperate with, a guild member for a guild charter and one outside the record's level window refused; else the offer and its line.
    fn offer(&self, target: Option<u64>) -> Vec<PetitionStep> {
        let (Some(item), Some(target)) = (self.state.petition, target.filter(|t| *t != 0)) else {
            return Vec::new();
        };
        let Some(me) = self.me.iter().next() else {
            return Vec::new();
        };
        let Some(record) = self.state.held_record() else {
            return Vec::new();
        };
        let unit = self
            .objects
            .entity(target)
            .and_then(|e| self.stores.get(e).ok())
            .filter(|s| s.0.is_unit());
        let Some(unit) = unit else {
            return vec![step(UiError::key("ERR_OUT_OF_RANGE"))];
        };
        if unit.0.object_type() != Some(benilla_protocol::messages::ObjectType::Player) {
            return Vec::new();
        }
        if target == self.me_guid() {
            return vec![step(lines::self_offer_line())];
        }
        if !crate::target::ring::can_cooperate_with_player(
            self.factions.as_deref(),
            Some(unit),
            Some(me),
        ) {
            return vec![step(UiError::key("ERR_GUILD_NOT_ALLIED"))];
        }
        let name = self
            .names
            .resolve(target, &self.commands)
            .unwrap_or_default()
            .to_string();
        if record.static_type == 0 && unit.0.player_guild_id() != 0 {
            return vec![step(UiError::s("ERR_ALREADY_IN_GUILD_S", name))];
        }
        let level = unit.0.unit_level().unwrap_or(0) as i64;
        if record.min_level > 0 && level < i64::from(record.min_level) {
            return vec![step(UiError::s("ERR_ARENA_TEAM_TARGET_TOO_LOW_S", name))];
        }
        if record.max_level > 0 && level > i64::from(record.max_level) {
            return vec![step(UiError::s("ERR_ARENA_TEAM_TARGET_TOO_HIGH_S", name))];
        }
        let team_size = if record.static_type == super::STATIC_TYPE_ARENA {
            (record.min_signatures + 1) as u32
        } else {
            0
        };
        let _ = self.commands.0.send(ClientCommand::OfferPetition {
            team_size,
            item,
            player: target,
        });
        vec![step(lines::offered_line(&name))]
    }
}

/// The designer's opener (`0x6d2500`), for `GetTabardInfo` with the registrar's guid `npc`: nothing while the designer is open on that guid (so two zero guids send nothing), the refusal while the player's `UNIT_FIELD_DISPLAYID` is not its `UNIT_FIELD_NATIVEDISPLAYID` (error `0x130`), else the packet.
pub(super) fn tabard_vendor_activate(
    npc: u64,
    designer: u64,
    player: &ObjectStore,
    commands: &NetCommands,
) -> Option<UiError> {
    if designer == npc {
        return None;
    }
    if player.0.unit_displayid() != player.0.unit_native_displayid() {
        return Some(UiError::key("ERR_EMBLEMERROR_NOTABARDGEOSET"));
    }
    let _ = commands.0.send(ClientCommand::TabardVendorActivate { npc });
    None
}

/// The arena banner the vendor keeps (`0xc0f7e8`, `0xacf7d8`..), all ones until a turn-in sets it.
pub(super) fn unset_banner() -> ArenaBannerChoice {
    ArenaBannerChoice {
        team_size: 0,
        background_color: u32::MAX,
        emblem_style: u32::MAX,
        emblem_color: u32::MAX,
        border_style: u32::MAX,
        border_color: u32::MAX,
    }
}
