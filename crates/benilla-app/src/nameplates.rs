//! Overhead unit names, the 1.12 world-text name system.
//!
//! - Geometry: a camera-facing glyph mesh in the world pass, depth-tested like the reference's name
//!   batch, so walls occlude names. Deviation: `AlphaMode::Blend` writes no depth where the
//!   reference does, because overlapping names resolving by sort order look the same.
//! - Colour: the render calls the ground ring's selector.
//! - Anchor: the posed PlayerName attachment, re-read every frame with no smoothing, as the
//!   reference does.
//! - Seat: the top line's baseline is at `anchor.z + lineCount*scale` and the block hangs down one
//!   pitch per line.
//! - Show gate (`0x729c70`): [`name_shown`] over the thirteen
//!   `UnitName*` bits of [`NameConfig`].
//! - Lines: an NPC's name and `<Subname>`; a player's `<AFK>`/`<DND>`/`<GM>` prefixes, the a4 title
//!   slot (`UnitNamePlayerPVPTitle`, bit `0x20`) and name, and `<Guild>`. The guild (a5) and
//!   subname (a6) slots share the format `"\n<%s>"` and are never both reached; a5 alone is
//!   CVar-gated.
//! - The a4 slot is 12340's chosen-title lookup (`0x72a290`): a player's `PLAYER_CHOSEN_TITLE` row
//!   from `CharTitles.dbc` with the name in its `%s`, gendered, and the `PVP_MEDAL<n>` line under
//!   it for a set `PLAYER_BYTES_3` byte 2; never a creature's.
//!
//! Not built: a7 and a8 have no cross-realm wire here.

use std::collections::HashMap;

use bevy::prelude::*;

use benilla_protocol::EntityKind;
use benilla_ui::script::{JustifyH, JustifyV, Outline, TitleRow};

use crate::entities::{overhead_anchor, BoneAttach, OverheadFallback};
use crate::names::NameCache;
use crate::net::{Guid, NetCommands, NetEntity, ObjectStore, Reputations, SelfPlayer};
use crate::target::{highlight_variant, CombatFlash, Factions, RingVariant, Selection};
use crate::ui_text::{layout_text_quads, FontSpec, Justify, TextSeat, UiFontAtlas};
use benilla_world::view::WorldCamera;

/// The height-scale law: `d > KNEE ? d/KNEE · RATE · FLOOR : FLOOR`, `d` the anchor's height above
/// the feet in world units.
const SCALE_FLOOR: f32 = 0.2; //
const SCALE_KNEE: f32 = 4.0; //
const SCALE_RATE: f32 = 1.5; //

/// The `UnitName*` CVars' bits in the name mask (`[0xd380a0]`): the registrar (`0x7e6150`) hands
/// each row its bit and the rows' change callback (`0x7e60e0`) sets or clears it.
pub(crate) mod name_bit {
    pub(crate) const OWN: u32 = 0x1;
    pub(crate) const NPC: u32 = 0x2;
    /// The guild line (`0x72d789`), not a whole name.
    pub(crate) const PLAYER_GUILD: u32 = 0x4;
    /// The a4 title slot (`0x72d749`), not a whole name.
    pub(crate) const PLAYER_PVP_TITLE: u32 = 0x8;
    pub(crate) const ENEMY_PLAYER: u32 = 0x10;
    pub(crate) const ENEMY_PET: u32 = 0x20;
    pub(crate) const ENEMY_TOTEM: u32 = 0x40;
    pub(crate) const FRIENDLY_PLAYER: u32 = 0x80;
    pub(crate) const FRIENDLY_PET: u32 = 0x100;
    pub(crate) const FRIENDLY_TOTEM: u32 = 0x200;
    pub(crate) const NON_COMBAT: u32 = 0x400;
    pub(crate) const ENEMY_GUARDIAN: u32 = 0x1000;
    /// Set and cleared by its row, but no reader tests it: a guardian on either side reads
    /// [`ENEMY_GUARDIAN`] (`0x729e54`).
    pub(crate) const FRIENDLY_GUARDIAN: u32 = 0x2000;
}

/// Each `UnitName*` row, lowercased, with its bit.
pub(crate) const NAME_CVARS: [(&str, u32); 13] = [
    ("unitnameown", name_bit::OWN),
    ("unitnamenpc", name_bit::NPC),
    ("unitnameplayerguild", name_bit::PLAYER_GUILD),
    ("unitnameplayerpvptitle", name_bit::PLAYER_PVP_TITLE),
    ("unitnameenemyplayername", name_bit::ENEMY_PLAYER),
    ("unitnameenemypetname", name_bit::ENEMY_PET),
    ("unitnameenemyguardianname", name_bit::ENEMY_GUARDIAN),
    ("unitnameenemytotemname", name_bit::ENEMY_TOTEM),
    ("unitnamefriendlyplayername", name_bit::FRIENDLY_PLAYER),
    ("unitnamefriendlypetname", name_bit::FRIENDLY_PET),
    ("unitnamefriendlyguardianname", name_bit::FRIENDLY_GUARDIAN),
    ("unitnamefriendlytotemname", name_bit::FRIENDLY_TOTEM),
    ("unitnamenoncombatcreaturename", name_bit::NON_COMBAT),
];

/// The name mask (`[0xd380a0]`), the `UnitName*` CVars as one word.
#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct NameConfig(pub(crate) u32);

impl NameConfig {
    pub(crate) fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
}

impl Default for NameConfig {
    /// The registered defaults: guild, title, enemy and friendly players and pets "1".
    fn default() -> Self {
        use name_bit::*;
        Self(
            PLAYER_GUILD
                | PLAYER_PVP_TITLE
                | ENEMY_PLAYER
                | ENEMY_PET
                | FRIENDLY_PLAYER
                | FRIENDLY_PET,
        )
    }
}

/// What the name gate (`0x729c70`) reads off a unit the viewer does not control.
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct NameSubject {
    pub(crate) is_player: bool,
    /// The current target (`[0xbd07b0]`), named whatever the CVars.
    pub(crate) is_target: bool,
    /// The player can cooperate with it (`0x729b30`).
    pub(crate) cooperates: bool,
    /// The player can attack it (`0x729740`).
    pub(crate) attackable: bool,
    /// `UNIT_FIELD_BYTES_1` byte 2 bit `0x2`.
    pub(crate) stealthed: bool,
    /// The controller, `CHARMEDBY` else `CREATEDBY`, is a player in view.
    pub(crate) player_controlled: bool,
    /// `CREATEDBY` set, `SUMMONEDBY` clear and flags bit `0x200` (`0x729dc9`).
    pub(crate) created_marker: bool,
    pub(crate) creature_type: u32,
    /// The creator read (`0x71b600`) answers: `CREATEDBY` with neither `CHARMEDBY` nor
    /// `SUMMONEDBY`.
    pub(crate) guardian: bool,
}

/// The name gate's answer for a unit past its targetable, plate/bubble and own-unit
/// legs. Not applied: the vehicle-charmer leg (`0x729e96`-`0x729ec2`), which names a vehicle a
/// cooperating driver rides by the friendly-pet bit.
pub(crate) fn name_shown(cfg: NameConfig, s: &NameSubject) -> bool {
    use name_bit::*;
    if s.is_target {
        return true;
    }
    let side = |friendly: bool, friend_bit: u32, enemy_bit: u32| {
        cfg.has(if friendly { friend_bit } else { enemy_bit })
    };
    // Critters (8) and non-combat pets (12).
    let critter = matches!(s.creature_type, 8 | 12);
    if s.is_player {
        return if s.cooperates {
            cfg.has(FRIENDLY_PLAYER)
        } else {
            cfg.has(ENEMY_PLAYER) && !s.stealthed
        };
    }
    if s.player_controlled {
        let friendly = !s.attackable;
        if !friendly && s.stealthed {
            return false;
        }
        if s.created_marker || critter {
            return cfg.has(NON_COMBAT);
        }
        if s.creature_type == 11 {
            return side(friendly, FRIENDLY_TOTEM, ENEMY_TOTEM);
        }
        if s.guardian {
            return cfg.has(ENEMY_GUARDIAN);
        }
        return side(s.cooperates, FRIENDLY_PET, ENEMY_PET);
    }
    if s.stealthed && s.attackable {
        return false;
    }
    if critter {
        return cfg.has(NON_COMBAT);
    }
    cfg.has(NPC)
}

/// The a1-a3 prefix slots of the line stack, the player object's vtable `+0x7c/+0x80/+0x84`: each
/// set `PLAYER_FLAGS` bit (vmangos `Player.h:316-318`) stacks in slot order with no space before
/// the name; the base unit object's slots are stubs, so NPCs never decorate. The tags are
/// `CHAT_FLAG_AFK/DND/GM` (`GlobalStrings.lua:534-536`). The own player's AFK slot also reads the
/// client-side mirror ([`drive_nameplates`]).
const FLAG_PREFIXES: [(u32, &str); 3] = [(0x2, "<AFK>"), (0x4, "<DND>"), (0x8, "<GM>")];

/// The name-line prefix for a player's `PLAYER_FLAGS`; empty when unflagged.
fn flag_prefix(player_flags: u32) -> String {
    FLAG_PREFIXES
        .iter()
        .filter(|(bit, _)| player_flags & bit != 0)
        .map(|(_, s)| *s)
        .collect()
}

/// What the a4 title slot resolves: `CharTitles.dbc`'s rows and the `PVP_MEDAL<n>` strings, which
/// the honor feed reads off each VM once ([`crate::ui_honor`]): the world-text pass runs without
/// the VM.
#[derive(Resource, Default)]
pub(crate) struct PvpNameStrings {
    /// The VM session the strings were read from; `None` before the first VM.
    session: Option<u64>,
    /// Moves when the strings or the rows change, so a line built from the old ones rebuilds.
    generation: u64,
    strings: HashMap<String, String>,
    rows: Vec<TitleRow>,
}

impl PvpNameStrings {
    /// The VM session the strings were read from; `None` before the first VM.
    pub(crate) fn session(&self) -> Option<u64> {
        self.session
    }

    /// A VM's strings, every key it defines; a missing one is the builder's own miss. The
    /// generation moves only when they differ, so a new VM with the same strings rebuilds no plate.
    pub(crate) fn set(&mut self, session: u64, strings: HashMap<String, String>) {
        self.session = Some(session);
        if strings != self.strings {
            self.strings = strings;
            self.generation += 1;
        }
    }

    /// `CharTitles.dbc`'s rows in file order.
    pub(crate) fn set_rows(&mut self, rows: Vec<TitleRow>) {
        if rows != self.rows {
            self.rows = rows;
            self.generation += 1;
        }
    }

    /// One held string, for the feed's tests.
    #[cfg(test)]
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.strings.get(key).map(String::as_str)
    }

    /// The chosen-title name for `key`; `None` for no title, where the plain name shows.
    fn decorated(&self, key: TitleKey, name: &str) -> Option<String> {
        benilla_ui::script::chosen_title_name(
            &self.rows,
            key.chosen,
            key.female,
            key.medal,
            name,
            |global| self.strings.get(global).cloned(),
        )
    }
}

/// The a4 inputs a live line was built with: the steady frame compares these fields, so a title
/// change, a CVar flip or new strings rebuild without reading the strings again.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TitleKey {
    /// `PLAYER_CHOSEN_TITLE`, a mask id.
    chosen: i32,
    /// `UNIT_FIELD_BYTES_0` byte 2 is 1 (`0x72a2f6`).
    female: bool,
    /// `PLAYER_BYTES_3` byte 2.
    medal: u8,
    /// The a4 bit (`0x20`) was set when the line was built.
    on: bool,
    /// The [`PvpNameStrings`] generation the line was built from.
    strings: u64,
}

/// The a4 inputs for one unit, cheap enough for the steady frame: `None` off a player or before
/// its descriptor arrives.
fn title_key(
    cfg: &NameConfig,
    strings: &PvpNameStrings,
    net: &NetEntity,
    store: Option<&ObjectStore>,
) -> Option<TitleKey> {
    let store = store?;
    (net.kind == EntityKind::Player).then(|| TitleKey {
        chosen: store.0.player_chosen_title(),
        female: store.0.unit_gender() == Some(1),
        medal: store.0.player_pvp_medal().unwrap_or(0),
        on: cfg.has(name_bit::PLAYER_PVP_TITLE),
        strings: strings.generation,
    })
}

/// The reference's main name line: the a1-a3 flag tags glued on with no separator, then the a4
/// slot, the chosen-title name where `key` is on and resolves, else the plain name.
fn name_line(flags: u32, key: Option<TitleKey>, strings: &PvpNameStrings, name: &str) -> String {
    let decorated = key
        .filter(|key| key.on)
        .and_then(|key| strings.decorated(key, name));
    let mut line = flag_prefix(flags);
    line.push_str(decorated.as_deref().unwrap_or(name));
    line
}

/// A live plate: its entity and stack, and the inputs the stack was built from, so a steady frame
/// compares fields and re-reads neither the name cache nor the install strings.
struct LiveName {
    plate: Entity,
    lines: Vec<String>,
    paint: NamePaint,
    /// The reference's line inputs at build time.
    flags: u32,
    name: String,
    bracketed: Option<String>,
    key: Option<TitleKey>,
}

/// The name's world scale for a unit whose overhead anchor sits `d` world units above its feet.
/// Shared with the raid-target marker ([`crate::raid_marks`]), which it raises.
pub(crate) fn height_scale(d: f32) -> f32 {
    if d > SCALE_KNEE {
        (d / SCALE_KNEE) * SCALE_RATE * SCALE_FLOOR
    } else {
        SCALE_FLOOR
    }
}

/// What a name line is painted with: the unit's highlight colour, the ring's own, which the
/// reference's name render reads (`0x725980` → `0x718ac0`): the resolver's class
/// ([`highlight_variant`]) or the combat flash.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum NamePaint {
    /// The resolver's answer.
    Variant(RingVariant),
    /// The combat-flash pulse, the highlight colour's first branch; its tint is rewritten each
    /// frame from [`CombatFlash::color`].
    Flash,
}

impl NamePaint {
    // `linear_rgb` passes the authored bytes raw into the gamma framebuffer: unlit writes
    // base_color as-is, and the frame decodes once at FFXGlow.
    fn color(self) -> Color {
        match self {
            Self::Variant(v) => v.color(),
            Self::Flash => Color::linear_rgb(1.0, 0.0, 0.0), // seed: the wave's G=0 endpoint
        }
    }
}

/// Materials per colour, meshes per line stack and the live plate per unit. The meshes bake glyph
/// UVs, so they die when the glyph sheet resets; the sheet's texture handle never changes.
#[derive(Resource, Default)]
pub(crate) struct Nameplates {
    materials: HashMap<NamePaint, Handle<StandardMaterial>>,
    meshes: HashMap<Vec<String>, Handle<Mesh>>,
    /// unit → its live plate and the inputs the stack was built from.
    live: bevy::ecs::entity::EntityHashMap<LiveName>,
    /// The atlas generation the meshes' UVs were built from; `None` before the first build.
    baked_from: Option<u64>,
}

impl Nameplates {
    /// Whether `unit` shows an overhead name (the reference's `unit+0xc7c` name object is live);
    /// the questgiver marker's raised (anim 190) bob keys on it.
    pub(crate) fn shows(&self, unit: Entity) -> bool {
        self.live.contains_key(&unit)
    }

    /// The line count of `unit`'s live name, for the raid marker's seat (one pitch above the
    /// block, or at the bare anchor when no name shows).
    pub(crate) fn line_count(&self, unit: Entity) -> Option<usize> {
        self.live.get(&unit).map(|live| live.lines.len())
    }
}

/// Marker on a plate entity (a world-pass billboard mesh, root-level, following its unit).
#[derive(Component)]
struct NamePlate;

/// Deviation: a mounted plate keeps only this fraction of its seat's oscillation about a slow
/// mean, because the raw seat, which the reference also rides, read more intense than the
/// reference side by side. `1.0` is the raw seat; on-foot plates are untouched.
const ROCK_KEEP: f32 = 0.7;
/// The mean tracker's rate (1/s): slow against the ~1 Hz gallop, fast enough to follow a stance.
const ROCK_MEAN_RATE: f32 = 1.5;
/// A residual this large (yd) is a stance change (mount swap, teleport): snap the mean to it.
const ROCK_SNAP: f32 = 1.0;

/// The raster size (logical px) of the mesh bake, which only sets crispness. Deviation: larger than
/// the reference's 32 px `UNIT_NAME_FONT` raster, which it magnifies, because a larger source is
/// crisper up close.
const BAKE_PX: f32 = 36.0;

/// Build one name-block mesh in pitch units: x centered, y up, line `i`'s baseline at
/// `lineCount - i`, one pitch being the font size. The baseline sits `TextEngine::ascent_ratio`
/// into the layout cell (the `[0x17c]` load param), as in the 2-D path.
fn build_name_mesh(atlas: &mut UiFontAtlas, lines: &[String]) -> Mesh {
    use bevy::asset::RenderAssetUsages;
    use bevy::mesh::{Indices, PrimitiveTopology};

    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let n = lines.len() as f32;
    // The baseline's place in the one-pitch cell, `round(size * asc/(asc+|desc|))`; it only has to
    // agree with `layout_text_quads`, since the baseline stays on the grid either way.
    let mut e = atlas.lock();
    // The size the glyphs are laid out at: `BAKE_PX` rounded to whole device pixels and back.
    let src = e.drawn_size(BAKE_PX);
    let baseline_frac =
        ((f64::from(src) * f64::from(e.ascent_ratio(None)) + 0.5).floor() / f64::from(src)) as f32;
    for (i, line) in lines.iter().enumerate() {
        let glyphs = layout_text_quads(
            &mut e,
            line,
            Rect::from_center_size(Vec2::ZERO, Vec2::ZERO),
            [1.0, 1.0, 1.0, 1.0], // the material tints; glyph alpha rides the texture
            Justify {
                h: JustifyH::Center,
                v: JustifyV::Middle,
            },
            0,
            FontSpec {
                path: None, // UNIT_NAME_FONT = Friz Quadrata, the default face
                height: Some(BAKE_PX),
                outline: Outline::None,
                alpha_gradient: None,
                spacing: 0.0,
            },
            // A world billboard: the glyphs are re-seated in pitch units below, off the UI grid.
            TextSeat::Exact,
        );
        // Recenter the ink box on x = 0, then normalize px → pitch units, flipping y-down px into
        // y-up locals within this line's band.
        let Some(bounds) = glyphs.iter().map(|q| q.rect).reduce(|a, b| a.union(b)) else {
            continue;
        };
        let cx = (bounds.min.x + bounds.max.x) * 0.5;
        // Line `i`'s baseline lands at local y = n - i; the layout puts it `baseline_frac` of a
        // cell below the cell top, so the cell top maps to n - i + baseline_frac.
        let line_top = n - i as f32 + baseline_frac;
        for q in &glyphs {
            let x0 = (q.rect.min.x - cx) / src;
            let x1 = (q.rect.max.x - cx) / src;
            let y0 = line_top - q.rect.min.y / src; // px top → local (higher)
            let y1 = line_top - q.rect.max.y / src; // px bottom → local (lower)
            let base = positions.len() as u32;
            positions.extend([[x0, y0, 0.0], [x1, y0, 0.0], [x1, y1, 0.0], [x0, y1, 0.0]]);
            let [tl, tr, br, bl] = q.uv.corners;
            uvs.extend([tl, tr, br, bl]);
            normals.extend([[0.0, 0.0, 1.0]; 4]);
            indices.extend([base, base + 2, base + 1, base, base + 3, base + 2]);
        }
    }
    drop(e);
    let mut mesh = Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    );
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, uvs);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
    mesh.insert_indices(Indices::U32(indices));
    mesh
}

/// Drive the plates: gate, resolve lines and colour, and rebuild the plate on change, in Update
/// where mesh churn is supported.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
pub(crate) fn drive_nameplates(
    mut commands: Commands,
    units: Query<(
        Entity,
        &NetEntity,
        &Guid,
        &Transform,
        Option<&ObjectStore>,
        // The viewer's own units (`0x71ffc0`): the active player or the unit it moves.
        (Has<SelfPlayer>, Has<crate::net::ActiveMover>),
        // Whether the body is drawn: a unit outside the scene has nothing to float a name over.
        Option<&InheritedVisibility>,
    )>,
    self_store: Query<(&Guid, &ObjectStore), With<SelfPlayer>>,
    // The optimistic AFK mirror, the own player's `<AFK>` override.
    mirror: Res<crate::ui_chat::AfkMirror>,
    // The show-gate inputs, one tuple param for Bevy's 16-param ceiling.
    gates: (
        Res<Selection>,
        Res<CombatFlash>,
        Option<Res<crate::player::CameraControl>>,
        Res<crate::vplates::VPlates>,
        Res<crate::chat_bubble::BubblesActive>,
        // The group roster: the reaction's party rung and the highlight's party colours.
        Res<crate::ui_party::GroupState>,
        // The friend list: the highlight's friend colour.
        Res<crate::ui_social::SocialState>,
        // The UnitName* CVar mask.
        Res<NameConfig>,
        // The title rows and medal strings ([`PvpNameStrings`]).
        Res<PvpNameStrings>,
        // A controller guid to its unit, for the player-controlled arm.
        Res<crate::net::GuidIndex>,
    ),
    names: Res<NameCache>,
    // The guild cache, the a5 line's text; `ResMut` because a miss sends `CMSG_GUILD_QUERY`.
    mut guilds: ResMut<crate::ui_guild::GuildState>,
    net_commands: Res<NetCommands>,
    factions: Option<Res<Factions>>,
    reputations: Res<Reputations>,
    camera: Query<&Transform, With<WorldCamera>>,
    mut atlas: Option<ResMut<UiFontAtlas>>,
    mut plates: ResMut<Nameplates>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    // The `overhead_anchor` inputs, for the spawn-frame seat only.
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform>,
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
) {
    let (selection, flash, rig, vplates, bubbles, group, social, name_cfg, pvp_strings, index) =
        gates;
    let (self_guid, own_store) = self_store
        .single()
        .map_or((None, None), |(g, s)| (Some(g.0), Some(s)));
    let (Ok(cam_tf), Some(atlas)) = (camera.single(), atlas.as_mut()) else {
        return;
    };
    // A glyph-sheet reset moves every cached UV.
    drop_stale_glyph_caches(&mut plates, atlas.generation, &mut commands);
    // The palette materials, once (they need the atlas image).
    if plates.materials.is_empty() {
        let image = atlas.image();
        for kind in RingVariant::ALL
            .map(NamePaint::Variant)
            .into_iter()
            .chain([NamePaint::Flash])
        {
            plates.materials.insert(
                kind,
                materials.add(StandardMaterial {
                    base_color: kind.color(),
                    base_color_texture: Some(image.clone()),
                    unlit: true,
                    // Depth-tested; Blend skips the depth write (the module doc's deviation).
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    // Sorted after every ordinary transparent (sign law in `sky_order`), as the
                    // reference draws world text after the liquid; water cannot cover a name.
                    depth_bias: benilla_world::sky_order::Rung::NAMEPLATE,
                    ..default()
                }),
            );
        }
    }
    // Written only while a flash is live, so an idle frame does not re-upload the material.
    if flash.unit.is_some() {
        if let Some(mat) = plates
            .materials
            .get(&NamePaint::Flash)
            .and_then(|h| materials.get_mut(h))
        {
            mat.base_color = flash.color;
        }
    }
    // The billboard facing: screen-aligned (the camera's own rotation), this frame's camera.
    let facing = cam_tf.rotation;
    let mut seen = bevy::ecs::entity::EntityHashSet::default();
    for (entity, net, guid, tf, store, (is_self, moved), drawn) in &units {
        // A name needs a drawn body; this precedes the name-show ladder.
        if !drawn.is_none_or(|v| v.get()) {
            continue;
        }
        if !matches!(net.kind, EntityKind::Player | EntityKind::Unit) {
            continue;
        }
        // No distance cull: the name's update, cull and build compare none.
        // The targetable test (`0x71a390`): a `NOT_SELECTABLE` unit is named only as our own pet.
        let untargetable = store.is_some_and(|s| {
            s.0.unit_flags() & 0x0200_0000 != 0
                && !(s.0.unit_created_by() == self_guid
                    && own_store.and_then(|o| o.0.unit_pet_guid()) == Some(guid.0))
        });
        // A V-key plate or chat bubble suppresses the name (`0x729cea`).
        let show = if untargetable || vplates.0.contains(&entity) || bubbles.0.contains(&entity) {
            false
        } else if is_self || moved {
            // The own bit, before the target rescue; no name over a fully faded first-person
            // avatar.
            name_cfg.has(name_bit::OWN)
                && (!is_self
                    || rig
                        .as_deref()
                        .map_or(1.0, crate::player::CameraControl::self_fade)
                        > 0.0)
        } else {
            let is_player = net.kind == EntityKind::Player;
            let fields = store.map(|s| &s.0);
            let controller =
                fields.and_then(|f| f.unit_charmed_by().or_else(|| f.unit_created_by()));
            let player_controlled = controller
                .and_then(|g| index.0.get(&g))
                .and_then(|&e| units.get(e).ok())
                .is_some_and(|u| u.1.kind == EntityKind::Player);
            let subject = NameSubject {
                is_player,
                is_target: selection.target == Some(entity),
                cooperates: crate::target::ring::can_cooperate_with_player(
                    factions.as_deref(),
                    store,
                    own_store,
                ),
                attackable: crate::target::ring::can_attack_from_player(
                    factions.as_deref(),
                    &reputations,
                    &*group,
                    store,
                    own_store,
                    is_player,
                ),
                stealthed: fields.is_some_and(|f| f.unit_is_stealthed()),
                player_controlled,
                created_marker: fields.is_some_and(|f| {
                    f.unit_created_by().is_some()
                        && f.unit_summoned_by().is_none()
                        && f.unit_flags() & 0x200 != 0
                }),
                creature_type: fields
                    .and_then(|f| f.object_entry())
                    .and_then(|e| names.creature_type(e))
                    .unwrap_or(0),
                guardian: fields.is_some_and(|f| {
                    f.unit_created_by().is_some()
                        && f.unit_charmed_by().is_none()
                        && f.unit_summoned_by().is_none()
                }),
            };
            name_shown(*name_cfg, &subject)
        };
        if !show {
            continue;
        }
        // Re-read by `peek`: resolve's `&mut`-tied return cannot outlive the subname read.
        if names.resolve_unit(guid.0, store, &net_commands).is_none() {
            continue;
        }
        let Some(name) = names.peek_unit(guid.0, store) else {
            continue;
        };
        // The player flag prefixes (a1-a3), glued onto the name with no space.
        let flags = if net.kind == EntityKind::Player {
            store.map_or(0, |s| s.0.player_flags())
        } else {
            0
        };
        // The AFK slot alone emits `<AFK>` for the active player while the mirror is set, whatever
        // the bit. Folded into `flags`, so the cache compare and [`name_line`] read the same bit.
        let flags = if is_self && mirror.is_afk() {
            flags | 0x2
        } else {
            flags
        };
        // The one `"\n<%s>"` slot: a6 subtitle for an NPC, a5 guild for a player (CVar-gated).
        let bracketed = match net.kind {
            EntityKind::Unit => benilla_protocol::guid::entry(guid.0)
                .and_then(|e| names.creature_subname(e))
                // vmangos sends "" for most templates: an empty subname is no line.
                .filter(|s| !s.trim().is_empty()),
            EntityKind::Player if name_cfg.has(name_bit::PLAYER_GUILD) => store
                .and_then(|s| crate::ui_guild::unit_guild_name(&s.0, &mut guilds, &net_commands)),
            _ => None,
        };
        // The highlight colour (`0x718ac0`), the ring's own: the combat flash first, while we
        // melee this unit, then the resolver. A unit whose fields have not streamed reads neutral.
        let color = if flash.unit == Some(entity) {
            NamePaint::Flash
        } else {
            NamePaint::Variant(store.map_or(RingVariant::Neutral, |s| {
                highlight_variant(
                    s,
                    factions.as_deref(),
                    &reputations,
                    &group,
                    own_store,
                    |g| social.is_friend(g),
                )
            }))
        };

        seen.insert(entity);
        let key = title_key(&name_cfg, &pvp_strings, net, store);
        match plates.live.get(&entity) {
            // The steady frame compares the inputs in place, allocating nothing; a rebuild from
            // these inputs is the stack `LiveName` holds.
            Some(live)
                if live.paint == color
                    && live.flags == flags
                    && live.key == key
                    && live.name == name
                    && live.bracketed.as_deref() == bracketed => {}
            stale => {
                if let Some(live) = stale {
                    if let Ok(mut e) = commands.get_entity(live.plate) {
                        e.despawn();
                    }
                }
                let mut lines = vec![name_line(flags, key, &pvp_strings, name)];
                if let Some(bracketed) = bracketed {
                    lines.push(format!("<{bracketed}>"));
                }
                debug!("nameplates: rebuild {entity} -> {lines:?} ({color:?})");
                let mesh = plates
                    .meshes
                    .entry(lines.clone())
                    .or_insert_with(|| meshes.add(build_name_mesh(atlas, &lines)))
                    .clone();
                let material = plates.materials[&color].clone();
                // The spawn-frame seat: placement can miss a plate whose unit despawns later
                // this Update, and a plate must never render at the origin.
                let anchor = overhead_anchor(
                    entity,
                    tf,
                    &anchor_q.0,
                    &anchor_q.1,
                    &anchor_q.2,
                    &anchor_q.3,
                    &anchor_q.4,
                );
                let scale = height_scale(anchor.y - tf.translation.y);
                let place = Transform {
                    translation: anchor,
                    rotation: facing,
                    scale: Vec3::splat(scale),
                };
                let plate = commands
                    .spawn((Mesh3d(mesh), MeshMaterial3d(material), place, NamePlate))
                    .id();
                plates.live.insert(
                    entity,
                    LiveName {
                        plate,
                        lines,
                        paint: color,
                        flags,
                        name: name.to_owned(),
                        bracketed: bracketed.map(str::to_owned),
                        key,
                    },
                );
            }
        }
    }
    // Plates whose unit despawned or gated off this frame.
    plates.live.retain(|unit, live| {
        if seen.contains(unit) {
            true
        } else {
            debug!("nameplates: drop {unit} {:?}", live.lines);
            if let Ok(mut e) = commands.get_entity(live.plate) {
                e.despawn();
            }
            false
        }
    });
}

/// Seat every live plate from this frame's propagated pose, so a moving name does not trail;
/// plates are roots, so writing `GlobalTransform` directly is exact.
#[allow(clippy::type_complexity)] // one Bevy system's full input set
fn place_nameplates(
    plates: Res<Nameplates>,
    camera: Query<&Transform, (With<WorldCamera>, Without<NamePlate>)>,
    units: Query<&Transform, (Without<NamePlate>, Without<WorldCamera>)>,
    mut plate_tfs: Query<(&mut Transform, &mut GlobalTransform), With<NamePlate>>,
    anchor_q: (
        Query<&BoneAttach>,
        Query<&benilla_world::rig_anim::RigPose>,
        Query<&OverheadFallback>,
        Query<&GlobalTransform, Without<NamePlate>>, // disjoint from the plate global write
        Query<(), With<crate::entities::mount::MountChild>>,
    ),
    trace: (
        Res<NameAnchorTrace>,
        Res<Time>,
        Query<(), With<crate::net::SelfPlayer>>,
    ),
    // unit → the slow mean of (anchor - root) while mounted, the rock-damping state.
    mut rock: Local<bevy::ecs::entity::EntityHashMap<Vec3>>,
) {
    let Ok(cam_tf) = camera.single() else {
        return;
    };
    let facing = cam_tf.rotation;
    let blend = 1.0 - (-ROCK_MEAN_RATE * trace.1.delta_secs()).exp();
    for (&unit, live) in plates.live.iter() {
        let (Ok(tf), Ok((mut ptf, mut pglobal))) = (units.get(unit), plate_tfs.get_mut(live.plate))
        else {
            continue; // spawned this frame and not yet flushed, or the unit is despawning
        };
        let raw = overhead_anchor(
            unit,
            tf,
            &anchor_q.0,
            &anchor_q.1,
            &anchor_q.2,
            &anchor_q.3,
            &anchor_q.4,
        );
        // Mounted: mean + ROCK_KEEP * residual, root-relative so the plate never lags.
        let anchor = if anchor_q.4.contains(unit) {
            let off = raw - tf.translation;
            let mean = rock.entry(unit).or_insert(off);
            if (off - *mean).length_squared() > ROCK_SNAP * ROCK_SNAP {
                *mean = off;
            } else {
                *mean += (off - *mean) * blend;
            }
            tf.translation + *mean + (off - *mean) * ROCK_KEEP
        } else {
            raw
        };
        if trace.0 .0 && trace.2.contains(unit) {
            // Root, seated (damped) anchor and camera per frame; `raw` is the undamped joint read.
            let (r, a, c) = (tf.translation, anchor, cam_tf.translation);
            info!(
                "NAME_TRACE t={:.4} root=({:.4},{:.4},{:.4}) anchor=({:.4},{:.4},{:.4}) cam=({:.3},{:.3},{:.3}) raw=({:.4},{:.4},{:.4})",
                trace.1.elapsed_secs(), r.x, r.y, r.z, a.x, a.y, a.z, c.x, c.y, c.z, raw.x, raw.y, raw.z
            );
        }
        let scale = height_scale(anchor.y - tf.translation.y);
        let place = Transform {
            translation: anchor,
            rotation: facing,
            scale: Vec3::splat(scale),
        };
        // Write only on a bit-level change, so a parked plate is not marked changed every frame.
        {
            let t = ptf.bypass_change_detection();
            if *t != place {
                *t = place;
                ptf.set_changed();
                *pglobal = GlobalTransform::from(place);
            }
        }
    }
    // Dismounted or despawned units drop their damping state (a re-mount starts fresh).
    rock.retain(|e, _| anchor_q.4.contains(*e));
}

/// Registers the plate driver (Update) and the per-frame placer (PostUpdate, after propagation,
/// before visibility culling).
pub(crate) struct NameplatesPlugin;

/// `WOW_PROBE_NAME_TRACE=1`: per-frame `NAME_TRACE` lines for the self player's plate seat.
#[derive(Resource)]
struct NameAnchorTrace(bool);

/// The rows' change callback (`0x7e60e0`): a `UnitName*` row sets or clears its bit.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut names: ResMut<NameConfig>) {
    let key = ev.key();
    let Some(&(_, bit)) = NAME_CVARS.iter().find(|(name, _)| *name == key.as_str()) else {
        return;
    };
    let mask = if ev.flag() {
        names.0 | bit
    } else {
        names.0 & !bit
    };
    names.set_if_neq(NameConfig(mask));
}

impl Plugin for NameplatesPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_cvar);
        app.insert_resource(NameAnchorTrace(
            std::env::var_os("WOW_PROBE_NAME_TRACE").is_some(),
        ))
        .init_resource::<Nameplates>()
        .init_resource::<NameConfig>()
        .init_resource::<PvpNameStrings>()
        // After targeting, V-key plates and chat bubbles: the gate reads their verdicts.
        .add_systems(
            Update,
            drive_nameplates
                .after(crate::target::TargetUpdate)
                .after(crate::vplates::VPlateSet)
                .after(crate::chat_bubble::BubbleSet),
        )
        .add_systems(Update, evict_name_meshes)
        .add_systems(
            PostUpdate,
            place_nameplates
                .after(bevy::transform::TransformSystems::Propagate)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
    }
}

/// Empty the UV-bearing caches when the glyph sheet resets, despawning the live plates too (they
/// hold the old `Mesh3d`); the materials survive. The first build only seeds.
fn drop_stale_glyph_caches(plates: &mut Nameplates, generation: u64, commands: &mut Commands) {
    if plates.baked_from == Some(generation) {
        return;
    }
    if plates.baked_from.is_some() {
        debug!(
            "nameplates: glyph sheet reset (generation {generation}) — dropping {} meshes and {} \
             live plates",
            plates.meshes.len(),
            plates.live.len(),
        );
        for live in plates.live.values() {
            if let Ok(mut e) = commands.get_entity(live.plate) {
                e.despawn();
            }
        }
        plates.live.clear();
        plates.meshes.clear();
    }
    plates.baked_from = Some(generation);
}

/// Drop the line-stack mesh dedup on a cross-map transition: the old map's names never return.
fn evict_name_meshes(
    mut changes: MessageReader<benilla_world::world_map::MapChange>,
    mut plates: ResMut<Nameplates>,
) {
    if changes.is_empty() {
        return;
    }
    changes.clear();
    plates.meshes.clear();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `>` not `>=` at the knee and `0.075*d` beyond: the jump at the knee is the reference's.
    #[test]
    fn height_scale_matches_the_byte_law() {
        assert_eq!(height_scale(1.8), 0.2, "human: the floor");
        assert_eq!(height_scale(4.0), 0.2, "knee is > not >=");
        let big = height_scale(8.0);
        assert!((big - 0.6).abs() < 1e-6, "8/4 · 1.5 · 0.2 = 0.6, got {big}");
        assert!(height_scale(4.1) > 0.2, "past the knee: the jump is real");
    }

    /// Bare concatenation in slot order AFK, DND, GM; other bits ignored.
    #[test]
    fn flag_prefix_matches_the_slot_law() {
        assert_eq!(flag_prefix(0), "");
        assert_eq!(flag_prefix(0x8), "<GM>");
        assert_eq!(flag_prefix(0x2), "<AFK>");
        assert_eq!(flag_prefix(0x4), "<DND>");
        assert_eq!(flag_prefix(0x8 | 0x2), "<AFK><GM>", "slot order, a1 first");
        assert_eq!(
            flag_prefix(0x2 | 0x4 | 0x8),
            "<AFK><DND><GM>",
            "all three stack — the client enforces no exclusivity"
        );
        assert_eq!(
            flag_prefix(0x10 | 0x20 | 0x1),
            "",
            "ghost/resting/leader don't decorate"
        );
    }

    /// The strings a VM defining `pairs` hands over, as the honor feed sets them, and two rows.
    fn strings_of(session: u64, pairs: &[(&str, &str)]) -> PvpNameStrings {
        let mut strings = PvpNameStrings::default();
        strings.set(session, set_of(pairs));
        strings.set_rows(vec![TitleRow {
            mask_id: 46,
            name: "%s the Explorer".into(),
            name1: "%s the Exploress".into(),
        }]);
        strings
    }

    fn set_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|&(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A streamed unit's kind and descriptor.
    fn unit(kind: EntityKind, fields: &[(u16, u32)]) -> (NetEntity, ObjectStore) {
        use benilla_protocol::messages::ObjectFields;
        (
            NetEntity {
                kind,
                display_id: None,
                scale: 1.0,
            },
            ObjectStore(ObjectFields::from_pairs(fields)),
        )
    }

    /// `UNIT_FIELD_BYTES_0` (36): race human, class warrior, male.
    const HUMAN_MALE: (u16, u32) = (36, 0x0101);
    /// `PLAYER_CHOSEN_TITLE`: mask 46.
    const EXPLORER: (u16, u32) = (benilla_protocol::field::FIELD_PLAYER_CHOSEN_TITLE, 46);

    /// The a4 inputs off the descriptor: the chosen title, the sex, the medal byte and the CVar; a
    /// creature has no a4 leg.
    #[test]
    fn title_key_reads_the_chosen_title_the_sex_and_the_cvar() {
        let cfg = NameConfig::default();
        let strings = strings_of(1, &[]);
        let (net, human) = unit(EntityKind::Player, &[HUMAN_MALE, EXPLORER, (195, 2 << 16)]);
        let key = title_key(&cfg, &strings, &net, Some(&human)).unwrap();
        assert_eq!(
            (key.chosen, key.female, key.medal, key.on),
            (46, false, 2, true)
        );
        let (_, orc) = unit(EntityKind::Player, &[(36, 0x010102), EXPLORER]);
        assert!(title_key(&cfg, &strings, &net, Some(&orc)).unwrap().female);
        let off = NameConfig(cfg.0 & !name_bit::PLAYER_PVP_TITLE);
        assert!(!title_key(&off, &strings, &net, Some(&human)).unwrap().on);
        let (creature, _) = unit(EntityKind::Unit, &[]);
        assert!(
            title_key(&cfg, &strings, &creature, Some(&human)).is_none(),
            "a creature has no a4"
        );
    }

    /// The composed main line, from the descriptor up: the reference glues the a1-a3 tags onto the
    /// a4 title slot with no separator; the CVar off, no title or a creature keep the plain name.
    #[test]
    fn the_name_line_glues_the_flag_tags_onto_the_titled_name() {
        let strings = strings_of(1, &[("PVP_MEDAL1", "Guardian")]);
        let line = |cfg: &NameConfig, (net, store): &(NetEntity, ObjectStore), flags, name| {
            name_line(
                flags,
                title_key(cfg, &strings, net, Some(store)),
                &strings,
                name,
            )
        };
        const AFK: u32 = 0x2;
        let cfg = NameConfig::default();
        let explorer = unit(EntityKind::Player, &[HUMAN_MALE, EXPLORER]);
        assert_eq!(line(&cfg, &explorer, AFK, "Bob"), "<AFK>Bob the Explorer");
        let off = NameConfig(cfg.0 & !name_bit::PLAYER_PVP_TITLE);
        assert_eq!(
            line(&off, &explorer, AFK, "Bob"),
            "<AFK>Bob",
            "the CVar off"
        );
        let plain = unit(EntityKind::Player, &[HUMAN_MALE]);
        assert_eq!(line(&cfg, &plain, 0, "Bob"), "Bob", "no title");
        let wolf = unit(EntityKind::Unit, &[HUMAN_MALE, EXPLORER]);
        assert_eq!(line(&cfg, &wolf, 0, "Young Wolf"), "Young Wolf");
        let female = unit(
            EntityKind::Player,
            &[(36, 0x01_0101), EXPLORER, (195, 1 << 16)],
        );
        assert_eq!(
            line(&cfg, &female, 0, "Alice"),
            "Alice the Exploress\nGuardian"
        );
    }

    /// The strings and rows move the key the plate cache compares only when they differ: the
    /// same strings from a new VM rebuild nothing, and a renamed title reaches the line.
    #[test]
    fn a_changed_title_set_moves_the_key_and_reaches_the_line() {
        let (net, store) = unit(EntityKind::Player, &[HUMAN_MALE, EXPLORER]);
        let cfg = NameConfig::default();
        let built = |strings: &PvpNameStrings| {
            let key = title_key(&cfg, strings, &net, Some(&store));
            (key, name_line(0, key, strings, "Bob"))
        };
        let mut strings = strings_of(1, &[]);
        let (boot_key, boot_line) = built(&strings);
        assert_eq!(boot_line, "Bob the Explorer");
        strings.set(2, set_of(&[]));
        assert!(built(&strings).0 == boot_key, "a new VM, the same strings");
        strings.set_rows(vec![TitleRow {
            mask_id: 46,
            name: "%s der Entdecker".into(),
            name1: String::new(),
        }]);
        let (key, line) = built(&strings);
        assert_eq!(line, "Bob der Entdecker", "the new row reaches the line");
        assert!(
            key != boot_key,
            "the cached key moves, so the live plate rebuilds"
        );
    }

    /// A parked plate's second placement leaves both change ticks alone; a moved unit still writes.
    #[test]
    fn a_parked_plates_seat_takes_no_write() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<Time>();
        world.insert_resource(NameAnchorTrace(false));
        world.spawn((Transform::default(), WorldCamera));
        let unit = world.spawn(Transform::from_xyz(3.0, -1.0, 5.0)).id();
        let plate = world
            .spawn((Transform::default(), GlobalTransform::default(), NamePlate))
            .id();
        let mut plates = Nameplates::default();
        plates.live.insert(
            unit,
            LiveName {
                plate,
                lines: vec!["Bob".to_string()],
                paint: NamePaint::Flash,
                flags: 0,
                name: "Bob".into(),
                bracketed: None,
                key: None,
            },
        );
        world.insert_resource(plates);

        // First seat: the anchor fallback (no model) is the unit's own translation.
        world.run_system_once(place_nameplates).unwrap();
        assert_eq!(
            world.get::<Transform>(plate).unwrap().translation,
            Vec3::new(3.0, -1.0, 5.0),
        );

        // A parked frame: bit-identical seat, so neither transform's tick may move.
        world.clear_trackers();
        world.run_system_once(place_nameplates).unwrap();
        let e = world.entity(plate);
        assert!(
            !e.get_ref::<Transform>().unwrap().is_changed(),
            "an unchanged seat must not re-mark the plate's transform"
        );
        assert!(
            !e.get_ref::<GlobalTransform>().unwrap().is_changed(),
            "…nor its global"
        );

        // A real move still lands, and marks.
        world.get_mut::<Transform>(unit).unwrap().translation.x += 2.0;
        world.clear_trackers();
        world.run_system_once(place_nameplates).unwrap();
        let e = world.entity(plate);
        assert_eq!(
            e.get::<Transform>().unwrap().translation,
            Vec3::new(5.0, -1.0, 5.0)
        );
        assert!(e.get_ref::<Transform>().unwrap().is_changed());
        assert!(e.get_ref::<GlobalTransform>().unwrap().is_changed());
    }

    /// A populated cache, as a re-bake finds it: one live plate, its mesh, its material.
    fn primed(generation: Option<u64>, unit: Entity, plate: Entity) -> Nameplates {
        let lines = vec!["Young Wolf".to_string()];
        let mut plates = Nameplates {
            baked_from: generation,
            ..Default::default()
        };
        plates.meshes.insert(lines.clone(), Handle::default());
        plates.materials.insert(NamePaint::Flash, Handle::default());
        plates.live.insert(
            unit,
            LiveName {
                plate,
                lines,
                paint: NamePaint::Flash,
                flags: 0,
                name: "Young Wolf".into(),
                bracketed: None,
                key: None,
            },
        );
        plates
    }

    /// The live plates hold the old `Mesh3d`, so a reset must despawn them, not only clear the map.
    #[test]
    fn a_sheet_reset_drops_the_meshes_and_the_live_plates_but_not_the_materials() {
        let mut world = World::new();
        let unit = world.spawn_empty().id();
        let plate = world.spawn_empty().id();
        let mut plates = primed(Some(7), unit, plate);

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            drop_stale_glyph_caches(&mut plates, 8, &mut commands);
        }
        queue.apply(&mut world);

        assert!(plates.meshes.is_empty(), "stale UVs must not survive");
        assert_eq!(
            plates.materials.len(),
            1,
            "the material binds a texture whose handle never moves — dropping it is pure churn"
        );
        assert!(plates.live.is_empty());
        assert!(
            world.get_entity(plate).is_err(),
            "a live plate holds the old mesh handle: clearing the map around it leaves the wrong \
             cells on screen"
        );
        assert_eq!(plates.baked_from, Some(8));
    }

    /// A reset on the first build would despawn the plates the frame they spawn.
    #[test]
    fn the_first_build_seeds_without_dropping_anything() {
        let mut world = World::new();
        let unit = world.spawn_empty().id();
        let plate = world.spawn_empty().id();
        let mut plates = primed(None, unit, plate);

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            drop_stale_glyph_caches(&mut plates, 0, &mut commands);
        }
        queue.apply(&mut world);

        assert_eq!(plates.meshes.len(), 1);
        assert_eq!(plates.live.len(), 1);
        assert!(world.get_entity(plate).is_ok());
        assert_eq!(plates.baked_from, Some(0));
    }

    #[test]
    fn an_unmoved_generation_is_a_no_op() {
        let mut world = World::new();
        let unit = world.spawn_empty().id();
        let plate = world.spawn_empty().id();
        let mut plates = primed(Some(3), unit, plate);

        let mut queue = bevy::ecs::world::CommandQueue::default();
        {
            let mut commands = Commands::new(&mut queue, &world);
            drop_stale_glyph_caches(&mut plates, 3, &mut commands);
        }
        queue.apply(&mut world);

        assert_eq!(plates.meshes.len(), 1);
        assert_eq!(plates.materials.len(), 1);
        assert_eq!(plates.live.len(), 1);
        assert!(world.get_entity(plate).is_ok());
    }

    /// The name gate's arms, each by its own bit: players by side and stealth, the
    /// player-owned totem, guardian and pet arms, critters, plain NPCs and the target rescue.
    #[test]
    fn each_unit_category_reads_its_own_name_bit() {
        use name_bit::*;
        let only = |bit: u32| NameConfig(bit);
        let friendly_player = NameSubject {
            is_player: true,
            cooperates: true,
            ..Default::default()
        };
        assert!(name_shown(only(FRIENDLY_PLAYER), &friendly_player));
        assert!(!name_shown(only(ENEMY_PLAYER), &friendly_player));
        let enemy_player = NameSubject {
            is_player: true,
            attackable: true,
            ..Default::default()
        };
        assert!(name_shown(only(ENEMY_PLAYER), &enemy_player));
        let stealthed = NameSubject {
            stealthed: true,
            ..enemy_player
        };
        assert!(
            !name_shown(only(ENEMY_PLAYER), &stealthed),
            "a stealthed enemy"
        );
        assert!(name_shown(
            only(0),
            &NameSubject {
                is_target: true,
                ..stealthed
            }
        ));
        let owned = |creature_type, guardian, attackable, cooperates| NameSubject {
            player_controlled: true,
            creature_type,
            guardian,
            attackable,
            cooperates,
            ..Default::default()
        };
        assert!(name_shown(
            only(FRIENDLY_TOTEM),
            &owned(11, false, false, false)
        ));
        assert!(name_shown(
            only(ENEMY_TOTEM),
            &owned(11, false, true, false)
        ));
        assert!(name_shown(
            only(FRIENDLY_PET),
            &owned(1, false, false, true)
        ));
        assert!(name_shown(only(ENEMY_PET), &owned(1, false, true, false)));
        assert!(
            name_shown(only(ENEMY_GUARDIAN), &owned(1, true, false, true)),
            "a friendly guardian reads the enemy-guardian bit"
        );
        assert!(!name_shown(
            only(FRIENDLY_GUARDIAN),
            &owned(1, true, false, true)
        ));
        assert!(name_shown(only(NON_COMBAT), &owned(12, false, false, true)));
        let critter = NameSubject {
            creature_type: 8,
            ..Default::default()
        };
        assert!(name_shown(only(NON_COMBAT), &critter));
        assert!(!name_shown(only(NPC), &critter));
        let npc = NameSubject {
            creature_type: 7,
            ..Default::default()
        };
        assert!(name_shown(only(NPC), &npc));
        assert!(
            !name_shown(NameConfig::default(), &npc),
            "UnitNameNPC boots off"
        );
    }
}
