//! The ground selection ring under the current target: the reference's `UnitSelectTexture.blp`
//! projected onto the terrain and WMO faces in its box, never doodads or GameObjects (flags
//! `0x200122`), additive, steady and at full brightness from the instant of selection, its bright
//! arc toward the camera. The vertical fade on wall and ledge pieces is matched to captures: the
//! reference's edge-fade ramp is untraced.

use benilla_formats::{load_faction_catalog, reputation_rank, FactionCatalog, Reaction};
use benilla_protocol::EntityKind;
use bevy::prelude::*;

use crate::net::{Guid, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::ui_party::GroupState;
use benilla_assets::{LockRecover, WorldAssets};
use benilla_world::decal::{DecalFrame, WorldDecal};
use benilla_world::particles::buffer::EffectVertex;

use super::click::{clear, deselect};
use super::{CombatFlash, Selection, SelectionRadius};
use crate::creature_anim::Engaged;
use benilla_world::view::WorldCamera;

/// The ring record: `verts` is rebuilt when the [`RingKey`] changes, `color` every frame.
#[derive(Resource, Default)]
pub(super) struct RingState {
    verts: Vec<EffectVertex>,
    key: RingKey,
    /// This frame's tint: the selector's colour or the combat flash's.
    color: Color,
    shown: bool,
}

/// The projection's inputs; while they hold still, the cached projection is reused.
#[derive(Default, PartialEq, Clone, Copy)]
struct RingKey {
    feet: Vec3,
    radius: f32,
    fade_angle: f32,
    surfaces: usize,
}

/// The ring texture; the draw waits until it loads.
#[derive(Resource)]
pub(super) struct RingAssets {
    texture: Handle<Image>,
}

/// The reference's selection-circle texture, loaded: a white ring, tinted per vertex.
const RING_TEXTURE: &str = "mpq://textures/unitselecttexture.blp";
/// Ring radius for a unit with no model: the reference's degenerate-box answer, 1.2.
const RING_FALLBACK_RADIUS: f32 = benilla_formats::DEGENERATE_RING_FOOTPRINT;
/// The highlight resolver's palette (`0x521bf0` with its third argument 1), not the nameplate's,
/// written as each decal vertex's diffuse: the reaction entries (`0xbd0c94`) red, orange
/// `0xFFFF8000`, yellow and green, dead grey `0xFF7F7F7F`, the soft blue `0xFF6060FF`
/// (`0xbd0c8c[1]`), party pale blue `0xFFAAAAFF` (`0xbd0c88`) or pale green `0xFFAAFFAA`
/// (`0xbd0c84`), and the friend colour `0xFF53C8FF` (`0xbd0c80`).
// `linear_rgb` passes the authored bytes raw: the framebuffer is gamma-encoded.
const RING_HOSTILE: Color = Color::linear_rgb(1.0, 0.0, 0.0);
const RING_UNFRIENDLY: Color = Color::linear_rgb(1.0, 0.502, 0.0);
const RING_NEUTRAL: Color = Color::linear_rgb(1.0, 1.0, 0.0);
const RING_FRIENDLY: Color = Color::linear_rgb(0.0, 1.0, 0.0);
const RING_PLAYER: Color = Color::linear_rgb(0.376, 0.376, 1.0);
const RING_DEAD: Color = Color::linear_rgb(0.498, 0.498, 0.498);
const RING_PARTY: Color = Color::linear_rgb(0.667, 0.667, 1.0); // 0xFFAAAAFF
const RING_PARTY_PVP: Color = Color::linear_rgb(0.667, 1.0, 0.667); // 0xFFAAFFAA
const RING_FRIEND: Color = Color::linear_rgb(0.325, 0.784, 1.0); // 0xFF53C8FF

/// The FactionTemplate.dbc catalog; absent if it failed to load, and the faction legs then read
/// neutral.
#[derive(Resource)]
pub(crate) struct Factions(FactionCatalog);

impl Factions {
    pub(crate) fn catalog(&self) -> &FactionCatalog {
        &self.0
    }

    /// Wrap a catalog, for tests; a running client builds the resource once, from the DBC.
    #[cfg(test)]
    pub(crate) fn from_catalog(catalog: FactionCatalog) -> Self {
        Self(catalog)
    }
}

/// The colour class [`super::highlight_variant`] resolves, shared by the ground ring and the
/// overhead name, which both read the unit's highlight colour (`0x718ac0`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum RingVariant {
    Hostile,
    Unfriendly,
    Neutral,
    Friendly,
    Player,
    Dead,
    Party,
    PartyPvp,
    /// A player on our friend list (`0xbd0c80`).
    Friend,
}

impl RingVariant {
    pub(crate) const ALL: [Self; 9] = [
        Self::Hostile,
        Self::Unfriendly,
        Self::Neutral,
        Self::Friendly,
        Self::Player,
        Self::Dead,
        Self::Party,
        Self::PartyPvp,
        Self::Friend,
    ];

    pub(crate) fn color(self) -> Color {
        match self {
            Self::Hostile => RING_HOSTILE,
            Self::Unfriendly => RING_UNFRIENDLY,
            Self::Neutral => RING_NEUTRAL,
            Self::Friendly => RING_FRIENDLY,
            Self::Player => RING_PLAYER,
            Self::Dead => RING_DEAD,
            Self::Party => RING_PARTY,
            Self::PartyPvp => RING_PARTY_PVP,
            Self::Friend => RING_FRIEND,
        }
    }
}

/// Load the ring texture and seed the ring record.
pub(super) fn setup_ring(mut commands: Commands, asset_server: Res<AssetServer>) {
    let texture = asset_server.load::<Image>(RING_TEXTURE);
    commands.insert_resource(RingAssets { texture });
    commands.init_resource::<RingState>();
}

/// Load FactionTemplate.dbc once the MPQ chain is open; on failure the resource stays absent.
pub(super) fn load_factions(mut commands: Commands, world_assets: Option<Res<WorldAssets>>) {
    let Some(world_assets) = world_assets else {
        return;
    };
    let mut chain = world_assets.chain.lock_recover();
    match load_faction_catalog(&mut chain) {
        Ok(catalog) => {
            info!("faction catalog: {} template rows", catalog.len());
            commands.insert_resource(Factions(catalog));
        }
        Err(e) => warn!("faction catalog unavailable, ring stays neutral: {e:#}"),
    }
}

/// Place, size and colour the ring under the current target, or hide it: radius the model's ring
/// footprint ([`SelectionRadius`]) times its scale, colour resolved every frame as faction can
/// change live.
#[allow(clippy::type_complexity)]
pub(super) fn update_ring(
    mut selection: ResMut<Selection>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    flash: Res<CombatFlash>,
    mut state: ResMut<RingState>,
    camera: Query<&GlobalTransform, With<WorldCamera>>,
    decals: WorldDecal,
    // Kept so a straight-down camera holds the last fade angle.
    mut fade_angle: Local<f32>,
    mut logged_guid: Local<Option<u64>>,
    // Last frame's (guid, dead), tracked every frame: a respawned creature reuses its guid, so
    // state armed only on a selection change would miss its second death.
    mut last_vitals: Local<Option<(u64, bool)>>,
    mut seam: crate::creature_anim::AttackSeam,
    // Net entities are roots, so `Transform` is this frame's world position. `.0` filters on
    // `Guid`, which a torn-down unit sheds at the teardown while its model fades on. `.1` gives a
    // mounted target the mount model's footprint at its rendered scale, as the reference's ring
    // cache (`+0xcf0`) recomputes from the mount.
    targets: (
        Query<
            (
                &Transform,
                Option<&SelectionRadius>,
                Option<&ObjectStore>,
                Option<&NetEntity>,
                Option<&crate::entities::mount::MountChild>,
            ),
            With<Guid>,
        >,
        Query<(&NetEntity, Option<&SelectionRadius>), With<crate::entities::mount::MountBody>>,
        // The group roster: the reaction's party rung and the highlight's party colour.
        Res<crate::ui_party::GroupState>,
        // The friend list: the highlight's friend colour.
        Res<crate::ui_social::SocialState>,
    ),
    self_store: Query<&ObjectStore, With<SelfPlayer>>,
    // Our auto-attack; both clear paths end it, as the reference's death and teardown edges do.
    engaged: Query<(), (With<Engaged>, With<SelfPlayer>)>,
) {
    let state = &mut *state;
    let hide = match selection.target {
        None => {
            *last_vitals = None;
            *logged_guid = None;
            true
        }
        Some(target) => match targets.0.get(target) {
            Ok((unit, sel_radius, store, net, mount_child)) => {
                // The footprint has no floor, as in the reference: a zero-bounds model already
                // measures the degenerate-box 1.2.
                let (local, mount_scale) = match mount_child.and_then(|mc| targets.1.get(mc.0).ok())
                {
                    Some((mnet, msel)) => (msel.map_or(RING_FALLBACK_RADIUS, |r| r.0), mnet.scale),
                    None => (sel_radius.map_or(RING_FALLBACK_RADIUS, |r| r.0), 1.0),
                };
                let radius = local * (unit.scale.x * mount_scale).max(0.01);
                *fade_angle = ring_fade_angle(&camera).unwrap_or(*fade_angle);
                let key = RingKey {
                    feet: unit.translation,
                    radius,
                    fade_angle: *fade_angle,
                    surfaces: decals.receiver_count(),
                };
                let projected = if state.shown && key == state.key {
                    !state.verts.is_empty()
                } else {
                    state.verts.clear();
                    state.key = key;
                    project_ring(&mut state.verts, &decals, unit.translation, radius, {
                        *fade_angle
                    })
                };
                // The unit's highlight colour (`0x718ac0`); a unit whose fields have not streamed
                // reads neutral, as the reaction does.
                let variant = store.map_or(RingVariant::Neutral, |s| {
                    super::highlight_variant(
                        s,
                        factions.as_deref(),
                        &reputations,
                        &targets.2,
                        self_store.single().ok(),
                        |g| targets.3.is_friend(g),
                    )
                });
                let is_player = net.is_some_and(|n| n.kind == EntityKind::Player);
                let is_dead = store.is_some_and(|s| s.0.unit_is_dead());
                // The alive-to-dead edge clears the target, as the reference's health handler does
                // (which sends `CMSG_SET_SELECTION 0`); a corpse selected afterwards stays
                // selected, gray.
                let died = is_dead
                    && selection
                        .guid
                        .is_some_and(|g| *last_vitals == Some((g, false)));
                *last_vitals = selection.guid.map(|g| (g, is_dead));
                if died {
                    // The selection set to nothing, so the dead target becomes the last one.
                    deselect(&mut selection, &mut seam, !engaged.is_empty());
                    *last_vitals = None;
                    state.shown = false;
                    state.verts.clear();
                    return;
                }
                if selection.guid != *logged_guid {
                    *logged_guid = selection.guid;
                    info!(
                        "target: guid {:?} ftpl {:?} (self ftpl {:?}) → {variant:?}{}{}",
                        selection.guid,
                        store.and_then(|s| s.0.unit_faction_template()),
                        self_store
                            .single()
                            .ok()
                            .and_then(|s| s.0.unit_faction_template()),
                        if is_player { " [player]" } else { "" },
                        if is_dead { " [dead→gray]" } else { "" },
                    );
                }
                // The combat flash, `0x718ac0`'s first branch, outranks the resolver.
                state.color = if flash.unit == Some(target) {
                    flash.color
                } else {
                    variant.color()
                };
                // No ground in the box hides the ring: the reference skips the draw.
                !projected
            }
            // No longer an object: clear and tell the server, as the reference's deactivate does on
            // both removal paths. This also drops a selected corpse at respawn, which the server
            // destroys before the fresh create.
            Err(_) => {
                clear(&mut selection, &mut seam, !engaged.is_empty());
                *last_vitals = None;
                *logged_guid = None;
                true
            }
        },
    };
    state.shown = !hide;
    if hide {
        state.verts.clear();
    }
}

/// The fade's camera-relative angle θ, turning the texture's faded side away from the camera as
/// the reference's camera-fed projector does. The texture is bright toward ring-local +Z and clear
/// toward −Z, and [`project_ring`] rotates the UVs by −θ, so `θ = −π/2 − atan2(f.z, f.x)` for the
/// camera's ground-projected forward `f`. `None` looking straight down.
fn ring_fade_angle(camera: &Query<&GlobalTransform, With<WorldCamera>>) -> Option<f32> {
    let cam = camera.single().ok()?;
    let f = cam.forward();
    let flat = Vec3::new(f.x, 0.0, f.z);
    if flat.length_squared() < 1e-6 {
        return None;
    }
    Some(-std::f32::consts::FRAC_PI_2 - flat.z.atan2(flat.x))
}

/// Project the ring through the reference's decal projector: the box is the texture square, yawed
/// by the fade angle, reaching two radii up and down. Vertex alpha is full within half a radius of
/// the feet and falls to 0 at the box's top and bottom. False when nothing is gathered.
fn project_ring(
    out: &mut Vec<EffectVertex>,
    decals: &WorldDecal<'_, '_>,
    feet: Vec3,
    radius: f32,
    fade_angle: f32,
) -> bool {
    let (sin, cos) = fade_angle.sin_cos();
    let vert = 2.0 * radius;
    let frame = DecalFrame {
        center: feet,
        sin,
        cos,
        min_x: -radius,
        max_x: radius,
        min_z: -radius,
        max_z: radius,
        min_y: -vert,
        max_y: vert,
    };
    decals.project(
        out,
        &frame,
        |p| ((vert - p.y.abs()) / (1.5 * radius)).clamp(0.0, 1.0),
        |x, z| frame.rect_uv(x, z),
    )
}

/// Draw the cached projection tinted, one additive draw on the ring rung above the blob shadows.
pub(super) fn push_ring(
    assets: Option<Res<RingAssets>>,
    state: Option<Res<RingState>>,
    cam: Query<Entity, With<WorldCamera>>,
    mut draw: benilla_world::particles::buffer::WorldEffectDraw,
) {
    let (Some(assets), Some(state)) = (assets, state) else {
        return;
    };
    let Ok(cam) = cam.single() else { return };
    if !state.shown || state.verts.is_empty() {
        return;
    }
    let tint = state.color.to_linear();
    let mut batch = draw
        .batch(cam, assets.texture.id())
        .additive()
        .anchored(state.key.feet)
        .rung(
            benilla_world::sky_order::Rung::RING,
            benilla_world::sky_order::Rung::DECAL_RASTER,
        );
    batch.extend(state.verts.iter().map(|v| EffectVertex {
        pos: v.pos,
        uv: v.uv,
        // The selector's colour as each vertex's diffuse, as in the reference; alpha is the fade.
        color: [tint.red, tint.green, tint.blue, v.color[3]],
    }));
    batch.tris();
}

/// The target's reaction toward our player as a rank `0..=6`, the direction every NPC colour uses
/// (ask `UnitReaction` with the unit as `this`). The player-vs-player rungs ([`pvp_rungs`]) decide
/// first: the unit itself is friendly, then, when both carry `UNIT_FIELD_FLAGS` bit 3, a duel, one
/// controller, the party and mutual FFA. Then: a reaction the server forces on the target's faction
/// answers first (`0x71f822`, the forced-reaction lookup at `0x5d06a0`); then a faction with a
/// reputation slot answers with our reputation rank, even in GM mode; any other goes to the
/// template comparator. Neutral when anything is missing. The reference caps that answer at 6, so
/// Revered and Exalted both read 6. A party rung lands on this same generic leg, `0x71f770` with
/// the unit as `this` and the player as the actor. Not applied: the contested guard and the summon
/// tail.
pub(crate) fn ring_reaction(
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> u8 {
    if let (Some(target), Some(own)) = (target_store, self_store) {
        if let PvpRung::Rank(rank) = pvp_rungs(&target.0, &own.0, party) {
            return rank;
        }
    }
    let resolved = (|| {
        let catalog = &factions?.0;
        let self_store = self_store?;
        let target_tpl = catalog.template(target_store?.0.unit_faction_template()?)?;
        // A forced reaction, for a player-controlled viewer (`0x71f7d5`).
        if self_store.0.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0 {
            if let Some(forced) = reputations.forced_reaction(target_tpl.faction) {
                return Some(u8::try_from(forced).unwrap_or(u8::MAX));
            }
        }
        // A reputation faction: our rank with it.
        if let Some(info) = catalog.reputation_faction(target_tpl.faction) {
            let standing = reputations
                .0
                .get(info.rep_index as usize)
                .map_or(0, |&(_flags, s)| s);
            let race = self_store.0.unit_race().unwrap_or(0);
            let class = self_store.0.unit_class().unwrap_or(0);
            return Some(reputation_rank(info.base_for(race, class) + standing));
        }
        // The template comparator: 1, 3 or 4 on the rank scale.
        let self_tpl = catalog.template(self_store.0.unit_faction_template()?)?;
        Some(target_tpl.reaction_toward(self_tpl) as u8)
    })();
    // A clamp (`cmp esi,6` / `jl` / `mov esi,6`) caps the reference's answer: Exalted's 7 reads 6.
    resolved.map_or(Reaction::Neutral as u8, |rank| rank.min(REACTION_CAP))
}

/// Revered, the highest rank `UnitReaction`'s generic leg answers.
const REACTION_CAP: u8 = 6;

/// `UNIT_FIELD_FLAGS` bit 3, player-controlled in behaviour: players and their pets carry it, wild
/// creatures do not. `UnitReaction` needs it on both sides before any player-vs-player rung, and
/// the attack check picks its reaction arm by it.
const UNIT_FLAG_PVP_ATTACKABLE: u32 = 1 << 3;

/// The duel rung of `UnitReaction`: hostile (1) between opposing teams, friendly (4) on one team,
/// `None` when no duel relates the two. It needs team and arbiter both: `PLAYER_DUEL_ARBITER` is
/// set at the challenge, `PLAYER_DUEL_TEAM` only when the countdown ends
/// (`Player::UpdateDuelFlag`), so the arbiter alone would turn the opponent red during the count.
fn duel_reaction(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> Option<u8> {
    match duel_rung(target, own) {
        DuelRung::Engaged { rank, .. } => Some(rank),
        _ => None,
    }
}

/// The group `UnitReaction`'s party rung asks about (`0x5129f0`, `0x512a00`): whether a guid other
/// than the active player's sits in our party slots or raid roster.
pub(crate) trait GroupMembers {
    fn is_member(&self, guid: u64) -> bool;
}

impl GroupMembers for GroupState {
    fn is_member(&self, guid: u64) -> bool {
        GroupState::is_member(self, guid)
    }
}

/// No group, for a system whose world may run without the party plugin.
pub(crate) struct NoGroup;

impl GroupMembers for NoGroup {
    fn is_member(&self, _guid: u64) -> bool {
        false
    }
}

/// The group a system holds, or [`NoGroup`] when its world runs without the party plugin.
pub(crate) fn party_of(group: Option<&GroupState>) -> &dyn GroupMembers {
    match group {
        Some(g) => g,
        None => &NoGroup,
    }
}

/// What `UnitReaction 0x7251c0`'s rungs ahead of its generic legs decide.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum PvpRung {
    /// A rung answered with this rank.
    Rank(u8),
    /// The party rung: the generic leg answers, skipping the FFA rung.
    Party,
    /// No rung matched: the generic legs answer.
    None,
}

/// `UnitReaction 0x7251c0`'s rungs ahead of its generic legs, in its order:
/// 1. the same unit is friendly (`0x7251c0` compares the objects first);
/// 2. with `UNIT_FIELD_FLAGS` bit 3 on both, a duel between the two decides;
/// 3. one controlling player for both, such as ours and our pet, is friendly;
/// 4. a controller in our party or raid sends the question to the generic leg (`0x71f770`),
///    which keeps a grouped player out of the FFA rung;
/// 5. both free-for-all is hostile.
///
/// The party rung asks of the pair whether one controller is the active player and the other a
/// group member (`0x5129f0`, `0x512a00`). Every caller passes the active player, or a unit he
/// controls, as one of the two, and the roster never lists him, so a controller in the roster is
/// the other side.
fn pvp_rungs(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
    party: &dyn GroupMembers,
) -> PvpRung {
    if same_unit(target, own) {
        return PvpRung::Rank(Reaction::Friendly as u8);
    }
    let player_controlled =
        |u: &benilla_protocol::ObjectFields| u.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    if !(player_controlled(target) && player_controlled(own)) {
        return PvpRung::None;
    }
    if let Some(rank) = duel_reaction(target, own) {
        return PvpRung::Rank(rank);
    }
    if let (Some(a), Some(b)) = (controller(target), controller(own)) {
        if a == b {
            return PvpRung::Rank(Reaction::Friendly as u8);
        }
        if party.is_member(a) || party.is_member(b) {
            return PvpRung::Party;
        }
    }
    if ffa_reaction(target, own) {
        return PvpRung::Rank(Reaction::Hostile as u8);
    }
    PvpRung::None
}

/// The same object: two stores carrying one `OBJECT_FIELD_GUID`, or one store passed twice.
fn same_unit(a: &benilla_protocol::ObjectFields, b: &benilla_protocol::ObjectFields) -> bool {
    std::ptr::eq(a, b) || a.object_guid().is_some_and(|g| b.object_guid() == Some(g))
}

/// The controlling player's guid (`0x718b70`): the charmer, else the creator, else the unit itself
/// when it is a player. The reference resolves a creator that is not a player one hop further;
/// without the object at hand that hop is not taken, and its guid stands.
fn controller(u: &benilla_protocol::ObjectFields) -> Option<u64> {
    u.unit_owner(benilla_protocol::messages::OwnerFallback::CreatedBy)
        .or_else(|| {
            (u.object_type() == Some(benilla_protocol::messages::ObjectType::Player))
                .then(|| u.object_guid())
                .flatten()
        })
}

/// The both-FFA rung of `UnitReaction` (`0x7253af`, `0x7253c1`): two player-controlled units both
/// carrying the free-for-all bit of `UNIT_FIELD_BYTES_2` byte 1 are hostile whatever their factions.
/// A plain PvP flag has no such rung: a same-faction player flagged for PvP stays friendly.
fn ffa_reaction(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> bool {
    let player_controlled =
        |u: &benilla_protocol::ObjectFields| u.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    player_controlled(target)
        && player_controlled(own)
        && target.unit_byte2_flags() & BYTE2_FFA_PVP != 0
        && own.unit_byte2_flags() & BYTE2_FFA_PVP != 0
}

/// Target `UNIT_FIELD_FLAGS` bits, any one of which makes the attack check refuse; their vanilla
/// names are unconfirmed, so none is used.
const CANNOT_BE_ATTACKED: u32 = 0x2 | 0x80 | 0x1_0000 | 0x10_0000 | 0x200_0000;
/// The cross-flag immunity bits the attack check reads on both sides, each against the other unit's
/// bit 3.
const IMMUNE_TO_PLAYER_CONTROLLED: u32 = 0x100;
const IMMUNE_TO_UNCONTROLLED: u32 = 0x200;
/// `UNIT_FIELD_BYTES_2` byte 1 (`[unit+0xd0]+0x1d1`), the PvP state the attack check (`0x729740`)
/// reads: flagged, the bit a contested zone sets, free-for-all and sanctuary (AzerothCore's
/// `UNIT_BYTE2_FLAG_*`).
const BYTE2_PVP: u8 = 0x1;
const BYTE2_CONTESTED: u8 = 0x2;
const BYTE2_FFA_PVP: u8 = 0x4;
const BYTE2_SANCTUARY: u8 = 0x8;
/// `PLAYER_FLAGS` `0x80000`, which refuses every attack by its holder (`0x72975d`).
const PLAYER_FLAGS_NO_ATTACK: u32 = 0x8_0000;

/// The local player's reaction toward a unit, with the player as `this`. This direction reaches leg
/// 3, which answers a reputation faction by the at-war bit alone, never the standing, so a
/// not-at-war neutral NPC is friendly here and neutral to [`ring_reaction`]: a friendly-category
/// plate with a yellow bar. A reaction the server forces on the target's faction answers ahead of
/// it (`0x725439`). Not applied, only ever making a unit friendlier: the charmed-player case.
pub(crate) fn reaction_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> u8 {
    // The player-vs-player rungs run first in both directions.
    if let (Some(target), Some(own)) = (target_store, self_store) {
        match pvp_rungs(&target.0, &own.0, party) {
            PvpRung::Rank(rank) => return rank,
            // `0x71f770` with the player as `this`: the target's controller is not the active
            // player, so its forced, at-war and standing legs never run, only the template
            // comparator, player toward unit.
            PvpRung::Party => {
                let reaction = (|| {
                    let catalog = &factions?.0;
                    let target_tpl = catalog.template(target.0.unit_faction_template()?)?;
                    let self_tpl = catalog.template(own.0.unit_faction_template()?)?;
                    Some(self_tpl.reaction_toward(target_tpl) as u8)
                })();
                return reaction.unwrap_or(Reaction::Neutral as u8);
            }
            PvpRung::None => {}
        }
    }
    let resolved = (|| {
        let catalog = &factions?.0;
        let target_tpl = catalog.template(target_store?.0.unit_faction_template()?)?;
        // A forced reaction, for a player-controlled viewer; capped as every answer is (`0x7254cb`).
        if self_store?.0.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0 {
            if let Some(forced) = reputations.forced_reaction(target_tpl.faction) {
                return Some(u8::try_from(forced.min(7)).unwrap_or(7));
            }
        }
        // Leg 3: a reputation faction answers by the at-war bit alone.
        if let Some(at_war) = at_war_with(catalog, reputations, target_tpl.faction) {
            return Some(if at_war {
                Reaction::Hostile as u8
            } else {
                Reaction::Friendly as u8
            });
        }
        // Leg 4: the template comparator, player toward unit.
        let self_tpl = catalog.template(self_store?.0.unit_faction_template()?)?;
        Some(self_tpl.reaction_toward(target_tpl) as u8)
    })();
    resolved.unwrap_or(Reaction::Neutral as u8)
}

/// Leg 3: whether we are at war with the `Faction.dbc` faction a unit's template names; `None` when
/// it has no reputation slot. The bit is the one the reputation pane's war checkbox writes, so
/// declaring war moves the plate, the cursor and attackability at once. `/reaction` prints it.
pub(crate) fn at_war_with(
    catalog: &benilla_formats::FactionCatalog,
    reputations: &Reputations,
    faction_id: u32,
) -> Option<bool> {
    let info = catalog.reputation_faction(faction_id)?;
    Some(
        usize::try_from(info.rep_index)
            .ok()
            .and_then(|i| reputations.0.get(i))
            .is_some_and(|&(flags, _)| flags & benilla_formats::faction_flags::AT_WAR != 0),
    )
}

/// The interact check from the local player, the world cursor's service gate: bit 25 of
/// `UNIT_FIELD_FLAGS` clear, some `UNIT_NPC_FLAGS`, and both reactions at least neutral,
/// [`ring_reaction`] and [`reaction_from_player`]. Not applied, each only ever making a unit less
/// interactable: the interact check's timed wrapper's gates (charm, alive) and the ghost leg.
pub(crate) fn can_interact_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> bool {
    let Some(target) = target_store else {
        return false; // fields not streamed yet
    };
    const UNIT_FLAG_NOT_SELECTABLE: u32 = 1 << 25;
    if target.0.unit_flags() & UNIT_FLAG_NOT_SELECTABLE != 0 || target.0.unit_npc_flags() == 0 {
        return false;
    }
    const NEUTRAL: u8 = Reaction::Neutral as u8;
    ring_reaction(factions, reputations, party, target_store, self_store) >= NEUTRAL
        && reaction_from_player(factions, reputations, party, target_store, self_store) >= NEUTRAL
}

/// The attack check from the local player (`0x729740`), every leg in the reference's order; the
/// V-plate category turns on it. Not a reaction threshold: an unflagged opposite-faction player on
/// a PvE realm is hostile yet not attackable, a same-faction duel opponent friendly yet attackable.
/// Not applied: the vehicle leg (`0x7298a9`–`0x7298db`) and the controller resolve (`0x729976`),
/// which takes the controller-less arm for a charmed unit.
pub(crate) fn can_attack_from_player(
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    target_is_player: bool,
) -> bool {
    let (Some(target), Some(own)) = (target_store, self_store) else {
        return false; // fields not streamed yet; the reference's null path refuses too
    };
    // The actor's own refusal (`0x72975d`).
    if own.0.player_flags() & PLAYER_FLAGS_NO_ATTACK != 0 {
        return false;
    }
    let (tflags, oflags) = (target.0.unit_flags(), own.0.unit_flags());
    // (a) The ghost gate (`0x729789`): only a creature whose type flags carry `0x2` may attack a
    // ghost player, so the local player never can.
    const PLAYER_FLAGS_GHOST: u32 = 0x10;
    if target_is_player && target.0.player_flags() & PLAYER_FLAGS_GHOST != 0 {
        return false;
    }
    // (b) Five target-flag disqualifiers.
    if tflags & CANNOT_BE_ATTACKED != 0 {
        return false;
    }
    // (c) The four cross-flag immunity legs.
    let (t_controlled, o_controlled) = (
        tflags & UNIT_FLAG_PVP_ATTACKABLE != 0,
        oflags & UNIT_FLAG_PVP_ATTACKABLE != 0,
    );
    if o_controlled && tflags & IMMUNE_TO_PLAYER_CONTROLLED != 0
        || !o_controlled && tflags & IMMUNE_TO_UNCONTROLLED != 0
        || oflags & IMMUNE_TO_PLAYER_CONTROLLED != 0 && t_controlled
        || oflags & IMMUNE_TO_UNCONTROLLED != 0 && !t_controlled
    {
        return false;
    }
    // (d) The three terminal arms, selected by the two bit-3 flags.
    let toward_target =
        || reaction_from_player(factions, reputations, party, target_store, self_store);
    let (tbyte, obyte) = (target.0.unit_byte2_flags(), own.0.unit_byte2_flags());
    let no_sanctuary = obyte & BYTE2_SANCTUARY == 0 && tbyte & BYTE2_SANCTUARY == 0;
    match (o_controlled, t_controlled) {
        // Neither player-controlled: hostile in either direction suffices.
        (false, false) => {
            toward_target() <= Reaction::Hostile as u8
                || ring_reaction(factions, reputations, party, target_store, self_store)
                    <= Reaction::Hostile as u8
        }
        // Both (`0x72994e`): a friendly reaction refuses; else a live duel; else the target's PvP
        // bit; else both free-for-all; else either side's contested bit. Sanctuary on either
        // side refuses the flag arms (`0x7299b5`–`0x729a0b`).
        (true, true) => {
            if toward_target() >= Reaction::Friendly as u8 {
                return false;
            }
            if matches!(duel_rung(&target.0, &own.0), DuelRung::Engaged { .. }) {
                return true;
            }
            if tbyte & BYTE2_PVP != 0 {
                no_sanctuary
            } else if obyte & BYTE2_FFA_PVP != 0 && tbyte & BYTE2_FFA_PVP != 0 {
                true
            } else {
                (obyte | tbyte) & BYTE2_CONTESTED != 0 && no_sanctuary
            }
        }
        // Mixed, the player against an ordinary NPC: refused when the uncontrolled side is in a
        // sanctuary (`0x729a2d`, `0x729a48`), else attackable when worse than friendly.
        _ => {
            !(o_controlled && tbyte & BYTE2_SANCTUARY != 0)
                && !(t_controlled && obyte & BYTE2_SANCTUARY != 0)
                && toward_target() < Reaction::Friendly as u8
        }
    }
}

/// A unit's `FactionTemplate` faction-group mask (`row + 0xc`), the one number the cooperation
/// check compares: 3 on every Alliance race row, 5 on every Horde one. `/reaction` prints it.
pub(crate) fn faction_group_mask(
    factions: Option<&Factions>,
    store: Option<&ObjectStore>,
) -> Option<u32> {
    let catalog = &factions?.0;
    Some(
        catalog
            .template(store?.0.unit_faction_template()?)?
            .group_mask,
    )
}

/// The cooperation check from the local player: equal faction-group masks, neither side charmed,
/// and not the same unit; it reads no party or raid state.
pub(crate) fn can_cooperate_with_player(
    factions: Option<&Factions>,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
) -> bool {
    let (Some(target), Some(own)) = (target_store, self_store) else {
        return false;
    };
    // The first leg: a unit never cooperates with itself.
    if std::ptr::eq(target, own) {
        return false;
    }
    if target.0.unit_charmed_by().is_some_and(|g| g != 0)
        || own.0.unit_charmed_by().is_some_and(|g| g != 0)
    {
        return false;
    }
    let resolved = (|| {
        let catalog = &factions?.0;
        let target_tpl = catalog.template(target.0.unit_faction_template()?)?;
        let self_tpl = catalog.template(own.0.unit_faction_template()?)?;
        Some(target_tpl.group_mask == self_tpl.group_mask)
    })();
    resolved.unwrap_or(false)
}

/// The V-plate category: `true` is the friendly bucket (Shift-V, bit `0x8`), `false` the enemy one
/// (V, bit `0x1`). A unit is friendly when the attack check refuses it, and a player only when the
/// cooperation check also passes; no reaction rank takes part.
pub(crate) fn plate_is_friendly(
    factions: Option<&Factions>,
    reputations: &Reputations,
    party: &dyn GroupMembers,
    target_store: Option<&ObjectStore>,
    self_store: Option<&ObjectStore>,
    target_is_player: bool,
) -> bool {
    let attackable = can_attack_from_player(
        factions,
        reputations,
        party,
        target_store,
        self_store,
        target_is_player,
    );
    if target_is_player {
        can_cooperate_with_player(factions, target_store, self_store) && !attackable
    } else {
        !attackable
    }
}

/// Why [`duel_reaction`]'s rung did or did not fire, with the values it judged; `/reaction` prints
/// it from this same walk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DuelRung {
    /// A side lacks `UNIT_FIELD_FLAGS` bit 3, so no player-vs-player rung runs.
    NotPlayerControlled { own: bool, target: bool },
    /// `PLAYER_DUEL_TEAM` is 0 on a side: no duel, or its countdown has not ended.
    NoTeam { own: u32, target: u32 },
    /// Both have a team but are not duelling each other, or the arbiter has not streamed.
    ArbiterMismatch { own: u64, target: u64 },
    /// The rung fires: 1 (hostile) on opposing teams, 4 (friendly) on one team.
    Engaged {
        rank: u8,
        own_team: u32,
        target_team: u32,
    },
}

/// The duel rung's walk, reporting the gate that decided.
pub(crate) fn duel_rung(
    target: &benilla_protocol::ObjectFields,
    own: &benilla_protocol::ObjectFields,
) -> DuelRung {
    let own_pc = own.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    let target_pc = target.unit_flags() & UNIT_FLAG_PVP_ATTACKABLE != 0;
    if !own_pc || !target_pc {
        return DuelRung::NotPlayerControlled {
            own: own_pc,
            target: target_pc,
        };
    }
    let own_team = own.player_duel_team();
    let target_team = target.player_duel_team();
    if own_team == 0 || target_team == 0 {
        return DuelRung::NoTeam {
            own: own_team,
            target: target_team,
        };
    }
    let arbiter = own.player_duel_arbiter();
    let target_arbiter = target.player_duel_arbiter();
    if arbiter == 0 || arbiter != target_arbiter {
        return DuelRung::ArbiterMismatch {
            own: arbiter,
            target: target_arbiter,
        };
    }
    DuelRung::Engaged {
        rank: if own_team == target_team {
            Reaction::Friendly as u8
        } else {
            Reaction::Hostile as u8
        },
        own_team,
        target_team,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        can_attack_from_player, can_cooperate_with_player, plate_is_friendly, ring_reaction,
        Factions, NoGroup, UNIT_FLAG_PVP_ATTACKABLE,
    };

    /// A reaction forced on the target's faction answers both directions ahead of every other leg
    /// for a player-controlled viewer, and not for one without the flag (`0x71f822`, `0x725439`).
    #[test]
    fn a_forced_reaction_answers_ahead_of_the_comparator() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        // A Chicken, template 31: neutral by the comparator.
        let chicken = ObjectStore(ObjectFields::from_pairs(&[(35, 31)]));
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1),
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let faction = factions.0.template(31).expect("template 31").faction;
        let forced = Reputations(Vec::new(), vec![(faction + 1, 4), (faction, 1)]);
        let both = |reps: &Reputations, me: &ObjectStore| {
            (
                ring_reaction(Some(&factions), reps, &NoGroup, Some(&chicken), Some(me)),
                super::reaction_from_player(
                    Some(&factions),
                    reps,
                    &NoGroup,
                    Some(&chicken),
                    Some(me),
                ),
            )
        };
        assert_eq!(both(&Reputations::default(), &me), (3, 3));
        assert_eq!(both(&forced, &me), (1, 1), "hostile by force");
        let uncontrolled = ObjectStore(ObjectFields::from_pairs(&[(35, 1)]));
        assert_eq!(
            both(&forced, &uncontrolled),
            (3, 3),
            "no flag 0x8, no forced leg"
        );
    }

    /// The emissary's rank stays neutral, which paints its bar yellow, while its plate category is
    /// friendly: the two run the reaction in opposite directions.
    #[test]
    fn the_plate_category_reproduces_the_reference_on_the_real_dbc() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        // Field indices: 35 = UNIT_FIELD_FACTIONTEMPLATE, 46 = UNIT_FIELD_FLAGS.
        let unit = |tpl: u32| ObjectStore(ObjectFields::from_pairs(&[(35, tpl)]));
        // Us: a player-controlled Human, faction template 1.
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1),
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let quiet = Reputations::default(); // nothing at war
        let category = |unit: &ObjectStore, reps: &Reputations| {
            plate_is_friendly(
                Some(&factions),
                reps,
                &NoGroup,
                Some(unit),
                Some(&me),
                false,
            )
        };
        let rank = |unit: &ObjectStore, reps: &Reputations| {
            ring_reaction(Some(&factions), reps, &NoGroup, Some(unit), Some(&me))
        };

        // A Chicken, template 31 with no reputation slot: neutral, attackable, the enemy bucket.
        let chicken = unit(31);
        assert!(!category(&chicken, &quiet), "a critter is enemy-category");
        assert_eq!(rank(&chicken, &quiet), 3, "and its bar is neutral yellow");

        // A League of Arathor Emissary, template 1577, reputation slot 53, not at war.
        let emissary = unit(1577);
        assert!(category(&emissary, &quiet), "friendly-category at neutral");
        assert_eq!(
            rank(&emissary, &quiet),
            3,
            "…while the BAR stays yellow: the two halves run the reaction in opposite \
             directions, and this disagreement is the reference's own"
        );

        // Booty Bay, template 120: the comparator says 3, so only the at-war leg makes it friendly.
        let goblin = unit(120);
        assert!(category(&goblin, &quiet), "Booty Bay: friendly, not at war");
        let mut slots = vec![(0u8, 0i32); 64];
        slots[1] = (benilla_formats::faction_flags::AT_WAR, 0); // Booty Bay = slot 1
        let at_war = Reputations(slots, Vec::new());
        assert!(!category(&goblin, &at_war), "at war → enemy category");
        assert!(
            category(&emissary, &at_war),
            "…and only that faction's own bit moves"
        );

        // A Stormwind guard, template 12, faction 72, slot 19: friendly by both routes.
        assert!(category(&unit(12), &quiet));
        assert_eq!(rank(&unit(12), &quiet), 4);
    }

    /// The reputation leg walks the ladder up to Revered and stops: the reference caps the rank at
    /// 6, so a Stormwind unit reads the same to a Revered and an Exalted human, while the ladder
    /// itself, which the reputation pane reads, still reaches 7.
    #[test]
    fn the_reputation_rank_is_capped_at_revered() {
        use crate::net::ObjectStore;
        use crate::target::{stormwind_fixture, HUMAN_WARRIOR};
        use benilla_protocol::field::FIELD_UNIT_FACTIONTEMPLATE;
        use benilla_protocol::ObjectFields;

        /// `UNIT_FIELD_BYTES_0`, absolute descriptor index.
        const BYTES_0: u16 = 36;
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let me = ObjectStore(ObjectFields::from_pairs(&[(BYTES_0, HUMAN_WARRIOR)]));
        // Both sides of the Honored, Revered and Exalted thresholds, and the wire's own ceiling.
        for (total, want) in [
            (8_999, 4),
            (9_000, 5),
            (20_999, 5),
            (21_000, 6),
            (41_999, 6),
            (42_000, 6),
            (42_999, 6),
        ] {
            let (factions, template, reps) = stormwind_fixture(&mut chain, total);
            let unit = ObjectStore(ObjectFields::from_pairs(&[(
                FIELD_UNIT_FACTIONTEMPLATE,
                template,
            )]));
            assert_eq!(
                ring_reaction(Some(&factions), &reps, &NoGroup, Some(&unit), Some(&me)),
                want,
                "standing {total}"
            );
        }
        assert_eq!(
            benilla_formats::reputation_rank(42_000),
            7,
            "the ladder is not capped"
        );
    }

    /// Cenarion Circle, faction 609 with reputation slot 36: not at war, its NPCs are neither
    /// attackable nor, without a service bit, interactable, while the rank stays neutral.
    #[test]
    fn a_not_at_war_cenarion_circle_npc_takes_no_cursor_at_neutral() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        // 35 = UNIT_FIELD_FACTIONTEMPLATE, 46 = UNIT_FIELD_FLAGS, 147 = UNIT_NPC_FLAGS.
        let npc = |tpl: u32, npc_flags: u32| {
            ObjectStore(ObjectFields::from_pairs(&[(35, tpl), (147, npc_flags)]))
        };
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1), // Human
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let quiet = Reputations::default(); // nothing at war
        let mut slots = vec![(0u8, 0i32); 64];
        slots[36] = (benilla_formats::faction_flags::AT_WAR, 0); // Cenarion Circle = slot 36
        let at_war = Reputations(slots, Vec::new());

        let attackable = |u: &ObjectStore, r: &Reputations| {
            can_attack_from_player(Some(&factions), r, &NoGroup, Some(u), Some(&me), false)
        };
        let interactable = |u: &ObjectStore, r: &Reputations| {
            crate::target::ring::can_interact_from_player(
                Some(&factions),
                r,
                &NoGroup,
                Some(u),
                Some(&me),
            )
        };

        // Template 996, faction 609, with no service bit.
        let silent = npc(996, 0);
        assert_eq!(
            ring_reaction(Some(&factions), &quiet, &NoGroup, Some(&silent), Some(&me)),
            3,
            "neutral standing — the ring and the bar are RIGHT and must not move"
        );
        assert!(
            !attackable(&silent, &quiet),
            "not at war → the attack check says no: the sword was the bug"
        );
        assert!(
            !interactable(&silent, &quiet),
            "and with no service bit it is not interactable either → the cursor CLEARS to Point"
        );

        // One that gossips keeps its speech bubble.
        let gossip = npc(996, crate::target::cursor_mode::npc_flags::GOSSIP);
        assert!(interactable(&gossip, &quiet), "gossip NPC still talks");
        assert!(!attackable(&gossip, &quiet));

        // At war: the gossiper leaves the ladder, the silent one takes the sword.
        assert!(
            attackable(&silent, &at_war),
            "at war → the sword is correct"
        );
        assert!(attackable(&gossip, &at_war));
        assert!(
            !interactable(&gossip, &at_war),
            "at war → no more gossip cursor; the reference leaves the ladder entirely"
        );
        assert_eq!(
            ring_reaction(Some(&factions), &at_war, &NoGroup, Some(&silent), Some(&me)),
            3,
            "the STANDING is still neutral — at-war does not touch the bar"
        );

        // A Chicken, template 31 with no reputation slot, still takes the sword.
        let chicken = npc(31, 0);
        assert!(
            attackable(&chicken, &quiet),
            "a neutral critter still takes the sword — the comparator, not at-war"
        );
        assert!(!interactable(&chicken, &quiet));
    }

    /// A player is friendly only if the cooperation check, faction-group-mask equality, also
    /// passes. Template 35, which vmangos's `.gm on` sets (`Player.cpp:2661`), is faction 31 with
    /// mask 0, so a GM-mode character leaves every player's friendly bucket while NPCs keep theirs.
    #[test]
    fn the_player_subject_category_on_the_real_dbc() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let reps = Reputations::default();
        let player = |tpl: u32| {
            ObjectStore(ObjectFields::from_pairs(&[
                (35, tpl),
                (46, UNIT_FLAG_PVP_ATTACKABLE),
            ]))
        };
        let category = |subject: &ObjectStore, me: &ObjectStore| {
            plate_is_friendly(
                Some(&factions),
                &reps,
                &NoGroup,
                Some(subject),
                Some(me),
                true,
            )
        };

        // Us: a Human, template 1, mask 3.
        let me = player(1);
        // A Gnome, template 115: another row with the same mask.
        assert!(
            category(&player(115), &me),
            "an Alliance player is friendly"
        );
        assert!(category(&player(1), &me), "…same race, same answer");
        // An Orc, template 2, mask 5: the enemy bucket, flagged or not.
        assert!(!category(&player(2), &me), "a Horde player is enemy");
        // A GM-mode player, template 35, mask 0, matches no one: enemy, though not attackable.
        assert!(!category(&player(35), &me), "a GM-mode player is enemy");
        assert!(
            !can_attack_from_player(
                Some(&factions),
                &reps,
                &NoGroup,
                Some(&player(35)),
                Some(&me),
                true
            ),
            "…and not because we can attack them",
        );

        // A GM-mode observer loses the friendly bucket for every player…
        let gm = player(35);
        for tpl in [1u32, 3, 4, 115, 2, 5, 6, 116] {
            assert!(
                !category(&player(tpl), &gm),
                "FT {tpl} is enemy-category to a GM-mode observer",
            );
        }
        // …while NPCs keep theirs: the cooperation check is never asked for an NPC.
        let npc = |tpl: u32| ObjectStore(ObjectFields::from_pairs(&[(35, tpl)]));
        let npc_category = |subject: &ObjectStore, me: &ObjectStore| {
            plate_is_friendly(
                Some(&factions),
                &reps,
                &NoGroup,
                Some(subject),
                Some(me),
                false,
            )
        };
        assert!(npc_category(&npc(12), &gm), "the guard keeps his bucket");
        assert!(!npc_category(&npc(14), &gm), "and the mob keeps his");

        // Nobody cooperates with themselves.
        assert!(!can_cooperate_with_player(
            Some(&factions),
            Some(&me),
            Some(&me)
        ));
    }

    /// The attack check's flag refusals beat a hostile reaction.
    #[test]
    fn an_unattackable_flag_beats_a_hostile_reaction() {
        use crate::net::{ObjectStore, Reputations};
        use benilla_protocol::ObjectFields;

        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let factions = Factions(benilla_formats::load_faction_catalog(&mut chain).expect("dbc"));
        let me = ObjectStore(ObjectFields::from_pairs(&[
            (35, 1),
            (46, UNIT_FLAG_PVP_ATTACKABLE),
        ]));
        let reps = Reputations::default();
        let mob = |flags: u32| ObjectStore(ObjectFields::from_pairs(&[(35, 14), (46, flags)]));

        // Template 14, Monster: hostile by the comparator.
        assert_eq!(
            ring_reaction(Some(&factions), &reps, &NoGroup, Some(&mob(0)), Some(&me)),
            1
        );
        assert!(
            !plate_is_friendly(
                Some(&factions),
                &reps,
                &NoGroup,
                Some(&mob(0)),
                Some(&me),
                false
            ),
            "a plain hostile mob is enemy-category"
        );
        // Each of these refusal bits alone flips the category.
        for bit in [0x2u32, 0x80, 0x1_0000, 0x10_0000] {
            assert!(
                plate_is_friendly(
                    Some(&factions),
                    &reps,
                    &NoGroup,
                    Some(&mob(bit)),
                    Some(&me),
                    false
                ),
                "flag {bit:#x} refuses the attack → friendly category"
            );
        }
        // Bit 8 (`0x100`) refuses because we are player-controlled: the cross-flag leg.
        assert!(plate_is_friendly(
            Some(&factions),
            &reps,
            &NoGroup,
            Some(&mob(0x100)),
            Some(&me),
            false
        ));
    }

    /// A torn-down object stops being an object at the teardown, not when its model has faded: both
    /// of the reference's routes (`SMSG_DESTROY_OBJECT`, the out-of-range block) reach one
    /// teardown, which clears a matching selection and sends `CMSG_SET_SELECTION 0` at once, while
    /// only the detached model fades on.
    mod teardown {
        use benilla_protocol::field::{
            FIELD_UNIT_FLAGS, FIELD_UNIT_HEALTH, FIELD_UNIT_LEVEL, FIELD_UNIT_MAXHEALTH,
        };
        use benilla_protocol::messages::ObjectType;
        use benilla_protocol::{EntityKind, ObjectFields, SessionEvent};
        use bevy::ecs::system::RunSystemOnce;
        use bevy::prelude::*;
        use crossbeam_channel::Receiver;

        use crate::net::{ClientCommand, Guid, GuidIndex, NetCommands, ObjectStore, SelfPlayer};
        use crate::target::{AttackPick, Selection};
        use benilla_world::model_fade::DespawnFade;

        const ME: u64 = 0x0000_0000_0000_0007;
        const MOB: u64 = 0xF130_0000_1234_0001;

        fn create(guid: u64) -> SessionEvent {
            SessionEvent::ObjectCreate {
                guid,
                kind: EntityKind::Unit,
                display_id: None,
                position: [3.0, 0.0, 0.0],
                orientation: 0.0,
                scale: 1.0,
                speeds: None,
                mover: None,
                transport_progress: None,
                transport: None,
                spline: None,
                vehicle: None,
                // `OBJECT_FIELD_TYPE` (2) 0x9, object and unit, as a creature's create carries it:
                // the selection set's selectable test refuses any other type.
                fields: ObjectFields::from_pairs(&[
                    (2, 0x9),
                    (FIELD_UNIT_HEALTH, 100),
                    (FIELD_UNIT_MAXHEALTH, 100),
                    (FIELD_UNIT_LEVEL, 9),
                ])
                .into_created(ObjectType::Unit),
            }
        }

        /// The built client with our avatar and one selected live mob, the write channel captured.
        fn client_with_selected_mob() -> (App, Entity, Receiver<ClientCommand>) {
            let mut app = crate::game_plugins::schedule_tests::headless_client();
            let (tx, rx) = crossbeam_channel::unbounded();
            app.insert_resource(NetCommands(tx));
            let world = app.world_mut();
            // Inserted by a Startup system on a real boot; no schedule runs here.
            world.init_resource::<super::super::RingState>();
            let me = world
                .spawn((
                    SelfPlayer,
                    Guid(ME),
                    Transform::default(),
                    ObjectStore(
                        // Player-controlled: a neutral mob is attackable with no faction catalog.
                        ObjectFields::from_pairs(&[
                            (FIELD_UNIT_HEALTH, 100),
                            (FIELD_UNIT_MAXHEALTH, 100),
                            (FIELD_UNIT_FLAGS, 0x8),
                        ])
                        .into_created(ObjectType::Player),
                    ),
                ))
                .id();
            world.resource_mut::<GuidIndex>().0.insert(ME, me);
            crate::net::handlers::dispatch(world, vec![create(MOB)]);
            let mob = world.resource::<GuidIndex>().0[&MOB];
            *world.resource_mut::<Selection>() = Selection {
                target: Some(mob),
                guid: Some(MOB),
                ..Default::default()
            };
            // Control: a live target keeps its selection through a ring pass.
            world
                .run_system_once(super::super::update_ring)
                .expect("the ring runs on the built client");
            assert_eq!(world.resource::<Selection>().guid, Some(MOB));
            // The create's name query is on the channel too; only a selection send matters.
            assert!(
                !rx.try_iter()
                    .any(|c| matches!(c, ClientCommand::SetSelection { .. })),
                "a live target is not deselected"
            );
            (app, mob, rx)
        }

        fn selection_cleared_at(teardown: SessionEvent) {
            let (mut app, mob, rx) = client_with_selected_mob();
            let world = app.world_mut();
            crate::net::handlers::dispatch(world, vec![teardown]);
            assert!(
                world.get_entity(mob).is_ok(),
                "the model outlives the object"
            );
            assert!(world.get::<DespawnFade>(mob).is_some(), "…and fades");
            world
                .run_system_once(super::super::update_ring)
                .expect("the ring runs on the built client");
            let sel = world.resource::<Selection>();
            assert_eq!(
                (sel.target, sel.guid),
                (None, None),
                "the selection ends with the object, not 2 s later with its model"
            );
            let sent: Vec<_> = rx.try_iter().collect();
            assert!(
                sent.iter()
                    .any(|c| matches!(c, ClientCommand::SetSelection { guid: 0 })),
                "the server is told at the teardown: {sent:?}"
            );
        }

        #[test]
        fn a_destroyed_target_clears_the_selection_at_the_destroy() {
            selection_cleared_at(SessionEvent::ObjectDestroyed(MOB));
        }

        #[test]
        fn a_streamed_out_target_clears_the_selection_at_the_stream_out() {
            selection_cleared_at(SessionEvent::ObjectsRemoved(vec![MOB]));
        }

        /// The nearest-enemy scan never picks a fading model.
        #[test]
        fn the_nearest_enemy_scan_never_picks_a_torn_down_object() {
            let (mut app, _mob, _rx) = client_with_selected_mob();
            let world = app.world_mut();
            let acquire = |world: &mut World| {
                *world.resource_mut::<Selection>() = Selection::default();
                world
                    .run_system_once(
                        |mut pick: AttackPick,
                         mut sel: ResMut<Selection>,
                         mut seam: crate::creature_anim::AttackSeam,
                         mut errors: ResMut<crate::ui_action::UiErrorKeys>| {
                            pick.target(None, &mut sel, &mut seam, &mut errors)
                        },
                    )
                    .expect("the scan runs on the built client")
            };
            assert_eq!(
                acquire(world),
                Some(MOB),
                "control: the live mob is acquired"
            );
            crate::net::handlers::dispatch(world, vec![SessionEvent::ObjectsRemoved(vec![MOB])]);
            assert_eq!(acquire(world), None, "the fading model is not a candidate");
        }

        /// A respawn destroys and re-creates the guid in one tick: the create is a new object, and
        /// the fading model does not answer to the guid.
        #[test]
        fn a_same_tick_recreate_is_a_fresh_object_and_the_old_model_is_nobody() {
            let (mut app, old, _rx) = client_with_selected_mob();
            let world = app.world_mut();
            crate::net::handlers::dispatch(
                world,
                vec![SessionEvent::ObjectDestroyed(MOB), create(MOB)],
            );
            let fresh = world.resource::<GuidIndex>().0[&MOB];
            assert_ne!(fresh, old, "a fresh entity for the fresh object");
            assert!(world.get::<DespawnFade>(fresh).is_none());
            let answering: Vec<Entity> = world
                .query::<(Entity, &Guid)>()
                .iter(world)
                .filter(|(_, g)| g.0 == MOB)
                .map(|(e, _)| e)
                .collect();
            assert_eq!(answering, [fresh], "one object answers to the guid");
        }
    }
}
