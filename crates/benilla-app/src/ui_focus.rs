//! The engine's focus clears. `FocusUnit` and `ClearFocus` live in the VM, which owns the guid;
//! here the engine clears it as 12340 does: `CGGameUI::BreakTarget` (`0x524350`) when a unit
//! leaves the object manager (`0x734fd0`), turns unselectable (`0x73c330`) or `SMSG_BREAK_TARGET`
//! names it (`0x526586`); `PetChangeHandler` (`0x6d1970`) for a departed pet; and `LeaveWorld`
//! (`0x528c30`). `BreakTarget` drops a matching target too.

use std::collections::HashMap;

use benilla_protocol::field::{FIELD_UNIT_CHARM, FIELD_UNIT_FLAGS, FIELD_UNIT_SUMMON};
use benilla_protocol::{SessionEvent, SessionEventKind};
use benilla_ui::script::UiScript;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::net::{FieldChanged, GuidIndex, NetHandlerApp, ObjectStore, SelfGuid, SelfPlayer};
use crate::target::DeselectGuid;
use crate::ui_party::GroupState;
use crate::ui_script::UiFeed;

/// `UNIT_FLAG_NOT_SELECTABLE`, whose rise breaks the target (`0x73c5b4`).
const UNIT_FLAG_NOT_SELECTABLE: u32 = 0x0200_0000;

/// What [`feed_focus`] remembers about the world: each watched owner's pet guids (charm, summon)
/// and whether our avatar was in the world.
#[derive(Default)]
struct FocusMemo {
    pets: HashMap<u64, [u64; 2]>,
    in_world: bool,
}

/// Guids whose focus the next feed clears.
#[derive(Resource, Default)]
pub(crate) struct FocusBreaks(Vec<u64>);

/// What the group exemption reads.
#[derive(SystemParam)]
pub(crate) struct GroupGate<'w, 's> {
    group: Res<'w, GroupState>,
    self_guid: Res<'w, SelfGuid>,
    index: Res<'w, GuidIndex>,
    stores: Query<'w, 's, &'static ObjectStore>,
}

impl GroupGate<'_, '_> {
    fn store(&self, guid: u64) -> Option<&ObjectStore> {
        self.stores.get(*self.index.0.get(&guid)?).ok()
    }

    /// `CGGameUI::IsRaidMemberOrPet` (`0x512a30`): a party slot or raid row, or its pet.
    fn member_or_pet(&self, guid: u64) -> bool {
        let group = &*self.group;
        let live = |m: u64| self.store(m).map(|s| &s.0);
        let party = (!group.slots_emptied)
            .then(|| group.party_slots())
            .into_iter()
            .flatten()
            .map(|m| m.guid)
            .any(|m| m == guid || group.party_pet_guid(m, live(m)) == Some(guid));
        party
            || crate::ui_party::raid_row_guids(group, self.self_guid.0)
                .into_iter()
                .any(|m| m == guid || group.raid_pet_guid(m, live(m)) == Some(guid))
    }

    /// `CGUnit_C::IsDuelingWith` (`0x71f5c0`), its player arm: one arbiter, opposite teams.
    fn dueling_us(&self, guid: u64) -> bool {
        let (Some(me), Some(other)) = (self.self_guid.0.and_then(|g| self.store(g)), self.store(guid))
        else {
            return false;
        };
        let (me, other) = (&me.0, &other.0);
        me.player_duel_arbiter() != 0
            && me.player_duel_arbiter() == other.player_duel_arbiter()
            && me.player_duel_team() != other.player_duel_team()
    }

    /// The exemption `BreakTarget`'s gated callers apply (`0x735127`, `0x5265be`): a group member
    /// or pet keeps its target and focus unless it is dueling us.
    fn breaks(&self, guid: u64) -> bool {
        !self.member_or_pet(guid) || self.dueling_us(guid)
    }
}

/// `CGGameUI::BreakTarget` (`0x524350`): the focus at the next feed, the target now.
fn break_target(guid: u64, breaks: &mut FocusBreaks, deselect: &mut MessageWriter<DeselectGuid>) {
    breaks.0.push(guid);
    deselect.write(DeselectGuid(guid));
}

/// `CGUnit_C::Disable` (`0x734fd0`), the unit leaving the object manager: the gated break. Its
/// target leg is the teardown's own deselect.
fn on_unit_left(In(ev): In<SessionEvent>, gate: GroupGate, mut breaks: ResMut<FocusBreaks>) {
    let guids = match ev {
        SessionEvent::ObjectDestroyed(guid) => vec![guid],
        SessionEvent::ObjectsRemoved(guids) => guids,
        _ => return,
    };
    breaks.0.extend(guids.into_iter().filter(|&g| gate.breaks(g)));
}

/// `SMSG_BREAK_TARGET` (`0x526586`): the gated break, which for a group member also needs its
/// object (`0x5265cc`).
fn on_break_target(
    In(ev): In<SessionEvent>,
    gate: GroupGate,
    mut breaks: ResMut<FocusBreaks>,
    mut deselect: MessageWriter<DeselectGuid>,
) {
    if let SessionEvent::BreakTarget { guid } = ev {
        if gate.breaks(guid) {
            break_target(guid, &mut breaks, &mut deselect);
        }
    }
}

/// Apply the owed clears and the field-edge ones: `UNIT_FLAG_NOT_SELECTABLE` rising
/// (`0x73c5b4`), a departed pet of ours or a party member's (`0x6d1a30`), and the world leaving
/// with a focus outside the group (`0x528e44`).
#[allow(clippy::too_many_arguments)]
fn feed_focus(
    script: Option<NonSendMut<UiScript>>,
    mut breaks: ResMut<FocusBreaks>,
    mut edges: MessageReader<FieldChanged>,
    mut deselect: MessageWriter<DeselectGuid>,
    gate: GroupGate,
    me: Query<(), With<SelfPlayer>>,
    mut memo: Local<crate::ui_script::VmMemo<FocusMemo>>,
) {
    let Some(mut script) = script else {
        breaks.0.clear();
        return;
    };
    let memo = memo.get(&script);
    for edge in edges.read() {
        if edge.unit_field(FIELD_UNIT_FLAGS)
            && (edge.old ^ edge.new) & edge.new & UNIT_FLAG_NOT_SELECTABLE != 0
        {
            break_target(edge.guid, &mut breaks, &mut deselect);
        }
        let pet_field = [FIELD_UNIT_CHARM, FIELD_UNIT_SUMMON]
            .iter()
            .any(|&f| edge.unit_field(f) || edge.unit_field(f + 1));
        let owner = edge.guid;
        let watched = Some(owner) == gate.self_guid.0
            || gate.group.party_slots().any(|m| m.guid == owner);
        if pet_field && watched {
            let now = gate.store(owner).map_or([0; 2], |s| {
                [s.0.unit_charm().unwrap_or(0), s.0.unit_summon().unwrap_or(0)]
            });
            let was = memo.pets.insert(owner, now).unwrap_or([0; 2]);
            for (old, new) in was.into_iter().zip(now) {
                // A pet that left and is no longer an object (`0x6d1a0e`).
                if old != 0 && old != new && !gate.index.0.contains_key(&old) {
                    breaks.0.push(old);
                    deselect.write(DeselectGuid(old));
                }
            }
        }
    }

    let focus = script.focus_guid();
    let here = !me.is_empty();
    if std::mem::replace(&mut memo.in_world, here) && !here && focus != 0 && !gate.member_or_pet(focus)
    {
        breaks.0.push(focus);
    }
    if breaks.0.drain(..).any(|g| g == focus) && focus != 0 {
        script.set_focus_guid(0);
    }
}

pub(crate) struct UiFocusPlugin;

impl Plugin for UiFocusPlugin {
    fn build(&self, app: &mut App) {
        use SessionEventKind as K;
        app.init_resource::<FocusBreaks>()
            .net_handler(K::ObjectDestroyed, on_unit_left)
            .net_handler(K::ObjectsRemoved, on_unit_left)
            .net_handler(K::BreakTarget, on_break_target)
            .add_systems(
                Update,
                feed_focus
                    .in_set(UiFeed)
                    .run_if(crate::ui_script::ingame_ui_up),
            );
    }
}
