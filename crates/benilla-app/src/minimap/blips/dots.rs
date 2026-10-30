//! The object blips of the reference's object walk (`0x57f7f0`): each streamed object within 0.8
//! of the view radius takes the first blip its rules give, one `ObjectIcons.blp` cell per blip, and
//! the blip draw (`0x580380`) draws them by blip in order, the same floor first. The party dots are
//! the group-blip draw's (`0x57ff70`) class cells of `PartyRaidBlips.blp`.

use std::collections::HashMap;

use bevy::math::Rect;
use bevy::prelude::*;

use benilla_assets::coords::bevy_to_wow;
use benilla_formats::{LockCatalog, ShapeshiftForm};
use benilla_protocol::EntityKind;
use benilla_ui::script::{TownKind, TownType};

use crate::creature_type::CreatureTypeSources;
use crate::go_templates::GameObjectTemplates;
use crate::names::NameCache;
use crate::net::{GuidIndex, NetEntity, ObjectStore, Reputations};
use crate::target::Factions;
use crate::ui_pass::{UiQuad, UiQuads, UvRect};

use super::{party_member_pos, BlipCtx, HoverName, ObjectHover, TrackedCandidates, BLIP_BASIS_PX};

/// Every object and party blip's quad, `0.0125` of the screen height × 1.6667 (`0x57dd16`): 16 px
/// on the 140.8-px basis, and the hover box too (`0x57c720`).
const BLIP_PX: f32 = 16.0;
/// `UNIT_DYNAMIC_FLAGS` bit 0x2, set on a Hunter's Mark victim for its caster; one of the unit
/// tracking check's two always-track clauses.
const UNIT_DYNFLAG_TRACK_UNIT: u32 = 0x2;
/// The creature `type_flags` bit the interact-while-dead check (`0x715e30`) reads.
const TYPE_FLAG_INTERACT_WHILE_DEAD: u32 = 0x80;
/// The profession-trainer `UNIT_NPC_FLAGS` bit: a town type tracking it adds the title line.
const NPC_FLAG_PROFESSION_TRAINER: u32 = 0x40;
/// `PLAYER_FLAGS` ghost bit.
const PLAYER_FLAGS_GHOST: u32 = 0x10;

/// The object blips, each its own cell of the 8×2 `ObjectIcons.blp` atlas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::minimap) enum Blip {
    /// A tracked player we can attack.
    AttackablePlayer = 2,
    /// A tracked player of the other faction we cannot attack.
    EnemyFactionPlayer = 3,
    /// A tracked player of our faction.
    FriendlyPlayer = 4,
    /// A tracked creature, hated or hostile.
    HostileNpc = 5,
    /// A tracked creature, unfriendly or neutral.
    NeutralNpc = 6,
    /// A tracked creature, friendly or better.
    FriendlyNpc = 7,
    /// A town type's NPC or object, or a tracked resource.
    Tracked = 8,
    /// Quest status 8, or 2 under trivial tracking: the gold `!`.
    QuestAvailable = 9,
    /// Quest status 10: the gold `?`.
    QuestReward = 10,
    /// Quest status 7, or 4 under trivial tracking: the blue `!`.
    QuestRepeatable = 11,
    /// An undiscovered flight master (`0x6d5fc0`): the green `!`.
    Interact = 13,
    /// Our own pet or charm, cell 15, which the atlas leaves blank.
    OwnMinion = 15,
}

impl Blip {
    /// The atlas cell, `i & 7` across and `i >> 3` down (`0x5832f0`).
    pub(in crate::minimap) fn cell(self) -> [f32; 4] {
        let i = self as u32;
        let (c, r) = ((i & 7) as f32, (i >> 3) as f32);
        [c / 8.0, (c + 1.0) / 8.0, r / 2.0, (r + 1.0) / 2.0]
    }
}

/// Our tracking masks and the track-stealthed bit (`PLAYER_FIELD_BYTES & 0x2`).
#[derive(Clone, Copy, Default)]
pub(in crate::minimap) struct SelfTracking {
    pub(in crate::minimap) creatures: u32,
    pub(in crate::minimap) resources: u32,
    pub(in crate::minimap) stealthed: bool,
}

/// What the classifier reads besides the object itself.
pub(in crate::minimap) struct BlipWorld<'a> {
    pub(in crate::minimap) me: Option<&'a ObjectStore>,
    pub(in crate::minimap) self_guid: Option<u64>,
    pub(in crate::minimap) tracking: SelfTracking,
    pub(in crate::minimap) town: Option<&'static TownType>,
    pub(in crate::minimap) statuses: &'a HashMap<u64, u32>,
    pub(in crate::minimap) names: &'a NameCache,
    pub(in crate::minimap) templates: &'a GameObjectTemplates,
    pub(in crate::minimap) locks: Option<&'a LockCatalog>,
    pub(in crate::minimap) forms: Option<&'a HashMap<u32, ShapeshiftForm>>,
    pub(in crate::minimap) factions: Option<&'a Factions>,
    pub(in crate::minimap) reputations: &'a Reputations,
    pub(in crate::minimap) group: &'a crate::ui_party::GroupState,
}

/// What an object's classification was computed from: its store's change tick, its flight-master
/// flag, and the stamp of everything else [`BlipWorld::classify`] reads.
type ClassKey = (Option<bevy::ecs::change_detection::Tick>, bool, u64);

/// Last frame's classifications by guid: an object whose key stands keeps its class, so a crowd
/// standing still is not re-classified every frame; the map keeps only the frame's candidates.
#[derive(Default)]
pub(in crate::minimap) struct ClassMemo {
    last: HashMap<u64, (ClassKey, Option<Classified>)>,
    next: HashMap<u64, (ClassKey, Option<Classified>)>,
}

impl ClassMemo {
    /// Start a frame: last frame's classes become the ones to reuse.
    pub(in crate::minimap) fn begin(&mut self) {
        std::mem::swap(&mut self.last, &mut self.next);
        self.next.clear();
    }

    /// `guid`'s class under `key`, computed by `classify` only when the key moved.
    fn classified(
        &mut self,
        guid: u64,
        key: ClassKey,
        classify: impl FnOnce() -> Option<Classified>,
    ) -> Option<Classified> {
        let class = match self.last.get(&guid) {
            Some(&(k, c)) if k == key => c,
            _ => classify(),
        };
        self.next.insert(guid, (key, class));
        class
    }
}

/// A classified object: its blip, and whether the tooltip adds its title.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::minimap) struct Classified {
    pub(in crate::minimap) blip: Blip,
    /// A profession trainer under its town type.
    pub(in crate::minimap) titled: bool,
}

impl From<Blip> for Classified {
    fn from(blip: Blip) -> Self {
        Self {
            blip,
            titled: false,
        }
    }
}

impl BlipWorld<'_> {
    fn trivial(&self) -> bool {
        self.town.is_some_and(|t| t.kind == TownKind::TrivialQuests)
    }

    /// The dead-or-ghost test on the active player.
    fn self_dead(&self) -> bool {
        self.me.is_some_and(|m| {
            m.0.unit_health().unwrap_or(1) == 0 || m.0.player_flags() & PLAYER_FLAGS_GHOST != 0
        })
    }

    /// The quest-status rungs units and GameObjects share.
    fn quest_blip(&self, guid: u64) -> Option<Blip> {
        use benilla_protocol::messages::dialog_status::*;
        match self.statuses.get(&guid).copied()? {
            REWARD => Some(Blip::QuestReward),
            AVAILABLE => Some(Blip::QuestAvailable),
            LOW_LEVEL_AVAILABLE if self.trivial() => Some(Blip::QuestAvailable),
            AVAILABLE_REP => Some(Blip::QuestRepeatable),
            LOW_LEVEL_AVAILABLE_REP if self.trivial() => Some(Blip::QuestRepeatable),
            _ => None,
        }
    }

    /// The object walk for one object; `None` draws nothing. `unknown_flight_master` is a flight
    /// master whose node we have not learned.
    pub(in crate::minimap) fn classify(
        &self,
        guid: u64,
        kind: EntityKind,
        store: Option<&ObjectStore>,
        unknown_flight_master: bool,
    ) -> Option<Classified> {
        match kind {
            EntityKind::GameObject => self.classify_object(guid, store?),
            EntityKind::Unit | EntityKind::Player => {
                self.classify_unit(guid, kind, store?, unknown_flight_master)
            }
            _ => None,
        }
    }

    fn classify_unit(
        &self,
        guid: u64,
        kind: EntityKind,
        store: &ObjectStore,
        unknown_flight_master: bool,
    ) -> Option<Classified> {
        let f = &store.0;
        if kind == EntityKind::Player && self.group.is_member(guid) {
            return None; // the group blips draw them
        }
        let owner = match f.unit_charmed_by() {
            Some(g) if g != 0 => g,
            _ => f.unit_summoned_by().unwrap_or(0),
        };
        if owner != 0 && Some(owner) == self.self_guid {
            return Some(Blip::OwnMinion.into());
        }
        let record = f.object_entry().and_then(|e| self.names.creature_record(e));
        let interact_while_dead =
            record.is_some_and(|r| r.type_flags & TYPE_FLAG_INTERACT_WHILE_DEAD != 0);
        if f.unit_health().unwrap_or(0) as i32 <= 0 && !interact_while_dead {
            return None;
        }
        if let Some(TownKind::Npc(mask)) = self.town.map(|t| t.kind) {
            if f.unit_npc_flags() & mask != 0
                && crate::target::can_interact(
                    Some(store),
                    self.factions,
                    self.reputations,
                    self.group,
                    self.me,
                )
            {
                return Some(Classified {
                    blip: Blip::Tracked,
                    titled: mask & NPC_FLAG_PROFESSION_TRAINER != 0,
                });
            }
        }
        if let Some(b) = self.quest_blip(guid) {
            return Some(b.into());
        }
        if unknown_flight_master {
            return Some(Blip::Interact.into());
        }
        if !self.can_track_unit(store) {
            return None;
        }
        let blip = if kind == EntityKind::Player {
            if crate::target::can_attack(
                Some(store),
                self.factions,
                self.reputations,
                self.group,
                self.me,
            ) {
                Blip::AttackablePlayer
            } else if self.pvp_side(store) == self.me.and_then(|m| self.pvp_side(m)) {
                Blip::FriendlyPlayer
            } else {
                Blip::EnemyFactionPlayer
            }
        } else {
            let reaction = crate::target::ring_reaction(
                self.factions,
                self.reputations,
                self.group,
                Some(store),
                self.me,
            );
            match reaction {
                0 | 1 => Blip::HostileNpc,
                2 | 3 => Blip::NeutralNpc,
                _ => Blip::FriendlyNpc,
            }
        };
        Some(blip.into())
    }

    /// The PvP faction index: the race's template side, compared, never named.
    fn pvp_side(&self, store: &ObjectStore) -> Option<u32> {
        let catalog = self.factions?.catalog();
        Some(catalog.race_template(store.0.unit_race()?)?.group_mask & 6)
    }

    /// The unit tracking check (`0x6dca00`).
    fn can_track_unit(&self, store: &ObjectStore) -> bool {
        if self.self_dead() || store.0.unit_is_untrackable() {
            return false;
        }
        let types = CreatureTypeSources {
            names: Some(self.names),
            forms: self.forms,
        };
        tracked_creature(
            self.tracking,
            || types.of(store),
            store.0.unit_dynamic_flags(),
            store.0.unit_is_stealthed(),
        )
    }

    fn classify_object(&self, guid: u64, store: &ObjectStore) -> Option<Classified> {
        let f = &store.0;
        let type_id = f.gameobject_type_id();
        let reaction =
            crate::target::cursor_mode::go_reaction(self.factions, f.gameobject_faction(), self.me);
        // The tracked and interact checks (`0x70f850`, `0x71f8b0`): not a ghost, reaction neutral.
        if let Some(TownKind::GameObject(wanted)) = self.town.map(|t| t.kind) {
            let ghost = self
                .me
                .is_some_and(|m| m.0.player_flags() & PLAYER_FLAGS_GHOST != 0);
            if type_id == wanted as i32 && !ghost && reaction.is_none_or(|r| r >= 3) {
                return Some(Blip::Tracked.into());
            }
        }
        if let Some(b) = self.quest_blip(guid) {
            return Some(b.into());
        }
        let lock = self.templates.get(guid).map_or(0, |t| t.lock_id);
        crate::go_templates::can_track_object(
            self.self_dead(),
            type_id,
            || {
                crate::target::cursor_mode::go_can_use(
                    f.gameobject_flags(),
                    f.gameobject_dynamic_flags(),
                    reaction,
                )
            },
            self.tracking.resources,
            lock,
            self.locks,
        )
        .then_some(Blip::Tracked.into())
    }
}

/// The unit tracking check's tracking half: `UNIT_DYNFLAG_TRACK_UNIT`, or our track-stealthed bit
/// with the unit's CREEP flag, else the creature type's bit `type − 1` in `PLAYER_TRACK_CREATURES`.
fn tracked_creature(
    tracking: SelfTracking,
    creature_type: impl FnOnce() -> u32,
    dynamic_flags: u32,
    unit_creeping: bool,
) -> bool {
    if dynamic_flags & UNIT_DYNFLAG_TRACK_UNIT != 0 || tracking.stealthed && unit_creeping {
        return true;
    }
    if tracking.creatures == 0 {
        return false;
    }
    let t = creature_type();
    (1..=32).contains(&t) && tracking.creatures & (1u32 << (t - 1)) != 0
}

/// Draw every object blip by blip in order, the same floor first, then the other floor grey
/// (`0xffb0b0b0`); our own pet's blank cell is not drawn but still names itself on hover.
///
/// The reference's same-WMO test is read as indoor containment: indoors, only indoor objects
/// draw; outdoors, an indoor object draws grey. Returns the count per atlas cell.
pub(in crate::minimap) fn emit_object_blips(
    ctx: &BlipCtx,
    world: &BlipWorld,
    candidates: &TrackedCandidates,
    icons: &Handle<Image>,
    player_indoors: bool,
    // By the object's guid and feet, so a caller can keep a still object's verdict.
    mut indoors_at: impl FnMut(u64, Vec3) -> bool,
    // Last frame's classifications, kept for an object whose store and world stamp stand.
    memo: &mut ClassMemo,
    world_stamp: u64,
    quads: &mut UiQuads,
    hover: &mut Vec<ObjectHover>,
) -> [u16; 16] {
    let cull = ctx.radius_yd * super::BLIP_EDGE_RATIO;
    let mut blips: Vec<(bool, Classified, u64, Vec2, Option<String>)> = Vec::new();
    for (guid, net, tf, store, fm) in candidates.iter() {
        let w = bevy_to_wow(tf.translation());
        if (w[0] - ctx.wx).powi(2) + (w[1] - ctx.wy).powi(2) > cull * cull {
            continue;
        }
        let unknown_fm = fm.is_some_and(|s| !s.known);
        let key = (
            store.as_ref().map(|s| s.last_changed()),
            unknown_fm,
            world_stamp,
        );
        let classified = memo.classified(guid.0, key, || {
            world.classify(guid.0, net.kind, store.as_deref(), unknown_fm)
        });
        let Some(c) = classified else {
            continue;
        };
        let inside = indoors_at(guid.0, tf.translation());
        if player_indoors && !inside {
            continue;
        }
        let grey = !player_indoors && inside;
        let title = c
            .titled
            .then(|| store.as_ref().and_then(|s| s.0.object_entry()))
            .flatten()
            .and_then(|e| world.names.creature_subname(e))
            .map(str::to_string);
        blips.push((grey, c, guid.0, ctx.center + ctx.offset(w), title));
    }
    blips.sort_by_key(|&(grey, c, ..)| (grey, c.blip));
    let size = Vec2::splat(ctx.side * (BLIP_PX / BLIP_BASIS_PX));
    let mut counts = [0u16; 16];
    for (grey, c, guid, at, title) in blips {
        counts[c.blip as usize] += 1;
        let rect = Rect::from_center_size(at, size);
        if c.blip != Blip::OwnMinion {
            let tint = if grey { 0xb0 as f32 / 255.0 } else { 1.0 };
            quads.overlays.push(UiQuad {
                rect,
                z_key: ctx.z,
                texture: Some(icons.clone()),
                uv: UvRect::from_tex_coords(c.blip.cell()),
                color: [tint, tint, tint, ctx.alpha],
                ..default()
            });
        }
        if ctx.cursor.is_some_and(|p| rect.contains(p)) {
            let name = match world.templates.get(guid) {
                Some(t) => HoverName::Known(t.name.clone()),
                None => HoverName::Guid(guid),
            };
            hover.push(ObjectHover { name, grey, title });
        }
    }
    counts
}

/// `PartyRaidBlips.blp`'s 8×4 cell for a class (`0x57ff70`): `class − 1` on the party's list,
/// `class + 15` on the raid's.
fn group_cell(class: u8, raid: bool) -> Option<[f32; 4]> {
    let i = u32::from(class).checked_sub(1)? + if raid { 16 } else { 0 };
    let (c, r) = ((i & 7) as f32, (i >> 3) as f32);
    (i < 32).then_some([c / 8.0, (c + 1.0) / 8.0, r / 4.0, (r + 1.0) / 4.0])
}

/// The in-range group dots, class-coloured, drawn before the object blips (`0x57ff70`): the raid's
/// list, then the party's above it. A member's class is its descriptor's, else the name query's;
/// a member a tracked vehicle stands for draws none.
#[allow(clippy::too_many_arguments)]
pub(in crate::minimap) fn emit_group_dots(
    ctx: &BlipCtx,
    group: &crate::ui_party::GroupState,
    possessors: &[u64],
    guids: &GuidIndex,
    unit_pos: &Query<&GlobalTransform, With<NetEntity>>,
    class_of: impl Fn(u64) -> Option<u8>,
    names: &NameCache,
    blips: &Handle<Image>,
    quads: &mut UiQuads,
) {
    let rows = super::raid_rows(group)
        .map(|m| (m, true))
        .chain(group.party_slots().map(|m| (m, false)));
    for (m, raid) in rows {
        if possessors.contains(&m.guid) {
            continue;
        }
        let Some((x, y)) = party_member_pos(m, group, guids, unit_pos) else {
            continue;
        };
        let d = ((x - ctx.wx).powi(2) + (y - ctx.wy).powi(2)).sqrt();
        if d / ctx.radius_yd > super::BLIP_EDGE_RATIO {
            continue; // out of range: the arrow pass drew it
        }
        let class = class_of(m.guid).or_else(|| names.player_traits(m.guid).map(|t| t.1));
        let Some(cell) = class.and_then(|c| group_cell(c, raid)) else {
            continue;
        };
        quads.overlays.push(UiQuad {
            rect: Rect::from_center_size(
                ctx.center + ctx.offset([x, y, 0.0]),
                Vec2::splat(ctx.side * (BLIP_PX / BLIP_BASIS_PX)),
            ),
            z_key: ctx.z,
            texture: Some(blips.clone()),
            uv: UvRect::from_tex_coords(cell),
            color: [1.0, 1.0, 1.0, ctx.alpha],
            ..default()
        });
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod class_memo_tests {
    use super::{Blip, ClassMemo, Classified};
    use bevy::ecs::change_detection::Tick;

    /// A still object classifies once; its store's tick, its flight-master flag or the world
    /// stamp moving classifies it again.
    #[test]
    fn a_still_object_keeps_its_class_until_a_key_moves() {
        let mut memo = ClassMemo::default();
        let calls = std::cell::Cell::new(0);
        let frame = |memo: &mut ClassMemo, key: (Option<Tick>, bool, u64)| {
            memo.begin();
            memo.classified(9, key, || {
                calls.set(calls.get() + 1);
                Some(Classified::from(Blip::FriendlyNpc))
            })
        };
        let key = (Some(Tick::new(5)), false, 1);
        assert_eq!(frame(&mut memo, key), Some(Blip::FriendlyNpc.into()));
        frame(&mut memo, key);
        assert_eq!(calls.get(), 1, "still: classified once");
        frame(&mut memo, (Some(Tick::new(6)), false, 1));
        assert_eq!(calls.get(), 2, "its store moved");
        frame(&mut memo, (Some(Tick::new(6)), false, 2));
        assert_eq!(calls.get(), 3, "the world moved");
        frame(&mut memo, (Some(Tick::new(6)), true, 2));
        assert_eq!(calls.get(), 4, "its flight-master flag moved");
    }
}
