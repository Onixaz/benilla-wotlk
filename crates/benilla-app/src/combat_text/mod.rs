//! Floating combat text, the reference's WORLDTEXTSTRING engine.
//!
//! A combat-log handler picks a category 0-9 and submits a string over the recipient. The text is
//! world-anchored: the unit's overhead anchor is snapshotted at spawn (`0x6c73f0` to `0x608640`:
//! the PlayerName attachment, slot 18, else `feet + scale × bbox_z × 1.25`), moved `z − 1/3`, and
//! re-projected every frame (`0x483ee0`), rising and fading per its row of `0xce8828`. Numbers
//! are bare `"%d"`, misses localized words; at most 4 texts per unit, a hard drop (`0x6c73f0`).
//! Damage over yourself is suppressed at the emitters (Gate A, `0x71f990`/`0x722340`); the XP
//! emitter (category 4) is self-anchored and skips it. Your own heals float green (`0x722440`).
//!
//! Colour: the emitters key `B` (no spell record, or `AttributesEx3` bit 15, the sign of byte
//! `SpellRec+0x25`) and `K` (the `0x7190a0` source class). Under the `CombatDamage` master, self
//! melee is white, self spell gold `0xFFFFDE00`, pet melee orange `0xFFFF8400` (`PetMeleeDamage`),
//! pet spell gold (`PetSpellDamage`), and any other source floats nothing. Crit does not recolour
//! and there is no school colour. Bit 15 is [`melee_styled`]'s (vmangos
//! `SPELL_ATTR_EX3_NORMAL_RANGED_ATTACK`), set on exactly 8 rows of the real DBC, the basic ranged
//! shots, so a Throw or Auto Shot
//! floats white off `SMSG_SPELLNONMELEEDAMAGELOG`. The word emitter `0x607140` runs the same law
//! and gates, so a spell's miss word is gold like its number.
//!
//! A travelling spell's word waits for the projectile: `SMSG_SPELL_GO`'s inline emit is skipped
//! when `Spell.dbc` Speed is nonzero, and the arrival calls the same emitter (`0x702fc0`).
//!
//! The rise is added to world z before projection; the block is h-centred with its bottom at the
//! projected point and clamped on-screen, but a point the projector rejects destroys the text and
//! frees its slot (`6c7d9d` to `6c6dda` to `0x6c86a0`). The size is constant with distance,
//! `round_half_away(v × √(W²+H²))` ([`text_px`], `0x5c6fa0`): at 1024×768 a number is 23 px, a
//! crit settles at 35 and pops to ~70. The face is [`DamageTextFont`], created with flags 0
//! (`6c8493`): no outline.
//!
//! The fade `0x6c82e0` replaces the colour's alpha byte every tick, so the table's packed alpha
//! never renders. The shadow is black, offset per axis by the viewport fraction at `0xce8804`
//! (`0x5c8710`), drawn down-right before the text, its alpha capped to the text's by `0x5cd650`.
//!
//! Each frame every live string claims bucket 1 of the shared SmartScreenRect solver
//! ([`crate::smart_rect`], `0x6c7cc0` to `0x509520`), each unit's slots walked descending
//! (`0x6c6e00`): the newest number seats first and the older one moves, re-derived every frame.
//!
//! The combat-log and spell handlers feed the spawns; the quads append to [`UiQuads`] after the
//! script extract ([`crate::ui_pass::UiQuadAppend`]).

mod law;

/// Re-exported for the emitter arms' tests; live code gets it through [`damage_color`].
#[cfg(test)]
pub(crate) use law::COLOR_SPELL_GOLD;
use law::{
    argb, claimed_box_px, fade_alpha, melee_text, scale_value, shadow_offset_px, text_px,
    CATEGORIES,
};
pub(crate) use law::{damage_color, melee_styled, miss_word, DamageSource, DamageTextGates};

use bevy::prelude::*;

use benilla_ui::script::{JustifyH, JustifyV, Outline};

use crate::entities::{overhead_anchor, BoneAttach, OverheadFallback};
use crate::ui_pass::{overlay_z, UiQuadAppend, UiQuads};
use crate::ui_text::{layout_text_quads, FontSpec, Justify, UiFontAtlas};
use benilla_world::view::WorldCamera;

/// Spawn one floating text over `anchor`. The producer applies Gate A, so a spawn here shows
/// unless the cap drops it.
#[derive(Message)]
pub(crate) struct CombatTextSpawn {
    pub(crate) anchor: Entity,
    pub(crate) text: String,
    /// Config category 0-9 ([`CATEGORIES`]).
    pub(crate) category: u8,
    /// The emitter override ([`damage_color`]); `None` is the row's default.
    pub(crate) color: Option<u32>,
}

/// One live WORLDTEXTSTRING. `pos` is a spawn-time snapshot, so a unit that walks away leaves its
/// numbers behind; `anchor` is kept for the per-unit cap.
struct WorldText {
    anchor: Entity,
    pos: Vec3,
    born: f64,
    text: String,
    category: u8,
    /// Resolved at spawn: the emitter override, else the category row's default.
    color: u32,
    /// The unit slot, 0-3, first free (`0x6c73f0`); seats walk slots descending (`0x6c6e00`).
    slot: u8,
}

/// The live texts of all units, pooled; the per-anchor slot check stands in for per-unit arrays.
#[derive(Resource, Default)]
pub(crate) struct WorldTexts(Vec<WorldText>);

/// The face of the floating numbers: the Lua global `DAMAGE_TEXT_FONT`, resolved once per world
/// session. `0x6c8470` reads its value (`0x6c847c`, the only read) through `FrameScript_GetText`
/// (`0x703bf0`) and `GetGlobalString` (`0x704350`), a plain `lua_gettable`. It is called at
/// `0x401620`, inside `0x401570` and after the UI load `0x401602`, so after `Fonts.xml` and every
/// non-LoadOnDemand addon's `ADDON_LOADED`, where addons assign it. The handle `[0xce8820]` has one
/// writer and `/reloadui` does not re-run `0x6c8470` (it sets `[0xb4b3f4]` for `0x48fbf0`), so a
/// later assignment lands at the next world entry. `None` draws in the default face; the
/// reference has no fallback face and draws nothing.
#[derive(Resource, Default)]
pub(crate) struct DamageTextFont(pub(crate) Option<String>);

/// Read `DAMAGE_TEXT_FONT` at the tail of the world-entry UI load (`0x6c8481`). `0x704350` takes
/// a string or number and leaves `""` otherwise, which the font factory rejects; empty is `None`.
pub(crate) fn read_damage_text_font(script: &benilla_ui::script::UiScript) -> DamageTextFont {
    let value: Option<String> = script.lua().globals().get("DAMAGE_TEXT_FONT").ok();
    DamageTextFont(value.filter(|v| !v.is_empty()))
}

/// The client's per-unit slot count (`PLAYERNAMEDESC` +0x20..+0x2c): a 5th concurrent text over
/// one unit is dropped outright.
const MAX_PER_UNIT: usize = 4;

/// World text draws beneath all UI, as the reference draws it in the world scene: the append
/// lane's bottom band ([`crate::ui_pass::overlay_z`]).
const Z_WORLD_TEXT: u64 = overlay_z::WORLD_TEXT;

/// The first free of the unit's 4 slots; `None` is a hard drop, no eviction (`0x6c73f0`). The
/// slot is also the seat-order key.
fn free_slot(texts: &[WorldText], anchor: Entity) -> Option<u8> {
    (0..MAX_PER_UNIT as u8).find(|s| !texts.iter().any(|t| t.anchor == anchor && t.slot == *s))
}

/// The per-frame engine: admit spawns, expire the dead and project the living into quads appended
/// to [`UiQuads`]. The [`UiQuadAppend`] set keeps the mesh rebuild from landing between the
/// script's replace and this append.
pub(crate) fn float_combat_text(
    mut spawns: MessageReader<CombatTextSpawn>,
    mut texts: ResMut<WorldTexts>,
    time: Res<Time>,
    transforms: Query<&Transform>,
    // This frame's Transform, not last frame's propagated GlobalTransform, so the text does not
    // lag a moving camera; the world camera is a root entity, so `GlobalTransform::from` is exact.
    camera: Query<(&Camera, &Transform), With<WorldCamera>>,
    mut atlas: Option<ResMut<UiFontAtlas>>,
    mut quads: ResMut<UiQuads>,
    // The overhead anchor (`0x608640`): the PlayerName attachment, else the bbox fallback.
    anchors: Query<&BoneAttach>,
    poses: Query<&benilla_world::rig_anim::RigPose>,
    fallbacks: Query<&OverheadFallback>,
    globals: Query<&GlobalTransform>,
    mounts: Query<(), With<crate::entities::mount::MountChild>>,
    // Claim bucket 1, rebuilt every pass; plates own bucket 0.
    mut bucket: Local<crate::smart_rect::SmartBucket>,
    // Absent in a bare test world, which draws the default face.
    font: Option<Res<DamageTextFont>>,
) {
    let now = time.elapsed_secs_f64();
    // Headless there is no camera: spawns and expiry run, nothing draws.
    let cam = camera
        .single()
        .ok()
        .map(|(c, pose)| (c, GlobalTransform::from(*pose)));
    let viewport = cam.as_ref().and_then(|(c, _)| c.logical_viewport_size());
    for spawn in spawns.read() {
        let Some(slot) = free_slot(&texts.0, spawn.anchor) else {
            continue; // the 4-slot hard drop
        };
        let Ok(tf) = transforms.get(spawn.anchor) else {
            continue; // anchor despawned before the spawn landed
        };
        // The spawn snapshot (`0x6c73f0`): the overhead anchor (`0x608640`), then `z − 1/3`.
        let overhead = overhead_anchor(
            spawn.anchor,
            tf,
            &anchors,
            &poses,
            &fallbacks,
            &globals,
            &mounts,
        );
        debug!(
            "fct: \"{}\" (cat {}) over {:?}",
            spawn.text, spawn.category, spawn.anchor
        );
        if benilla_assets::trace::enabled() {
            let via_attach = anchors
                .get(spawn.anchor)
                .is_ok_and(|a| a.points.contains_key(&crate::entities::ATTACH_OVERHEAD));
            benilla_assets::trace::line(
                "fct",
                &format!(
                    "spawn \"{}\" cat={} anchor={:?} pos=({:.3},{:.3},{:.3}) attach18={}",
                    spawn.text,
                    spawn.category,
                    spawn.anchor,
                    overhead.x,
                    overhead.y,
                    overhead.z,
                    via_attach
                ),
            );
        }
        let pos = overhead - Vec3::Y / 3.0;
        texts.0.push(WorldText {
            anchor: spawn.anchor,
            pos,
            born: now,
            text: spawn.text.clone(),
            category: spawn.category,
            color: spawn
                .color
                .unwrap_or(CATEGORIES[spawn.category as usize].color),
            slot,
        });
    }
    texts
        .0
        .retain(|t| ((now - t.born) * 1000.0) < CATEGORIES[t.category as usize].dur_ms as f64);
    if texts.0.is_empty() {
        return;
    }
    let (Some((cam, cam_tf)), Some(atlas)) = (cam, atlas.as_mut()) else {
        return; // headless or pre-world: age, draw nothing
    };
    let Some(viewport) = viewport else {
        return;
    };
    // The per-frame reset (`0x509500`), then seats per unit in creation order (the anchor id
    // here), slots descending (`0x6c6e00`): the newest number seats first, the older is pushed.
    bucket.clear();
    let mut order: Vec<usize> = (0..texts.0.len()).collect();
    order.sort_by_key(|&i| (texts.0[i].anchor, std::cmp::Reverse(texts.0[i].slot)));
    // A rejected anchor destroys the text and frees its slot (`6c7d9d` to `6c6dda` to
    // `0x6c86a0`); reaped after the walk, which holds the list.
    let mut culled: Vec<usize> = Vec::new();
    for i in order {
        let t = &texts.0[i];
        let cat = &CATEGORIES[t.category as usize];
        let elapsed_ms = ((now - t.born) * 1000.0) as f32;
        let life = elapsed_ms / cat.dur_ms;
        // The rise enters world z before projection (`6c7d29`, projected at `6c7d96`).
        let world = t.pos + Vec3::Y * (cat.rise * life);
        let Some(screen) = crate::ui_pass::project_overlay(cam, &cam_tf, world, viewport) else {
            culled.push(i); // behind the camera or off the viewport
            continue;
        };
        // Constant with distance: the worldtext path has no depth term.
        let size_value = scale_value(t.category, life);
        let target_px = text_px(size_value, viewport);
        // Deviation: the reference stretches a raster of at most 32 px; each whole-pixel size is
        // rasterized here, so the crit pop stays crisp.
        let mut e = atlas.lock();
        let mut glyphs = layout_text_quads(
            &mut e,
            &t.text,
            Rect::from_center_size(screen, Vec2::ZERO),
            argb(t.color),
            Justify {
                h: JustifyH::Center,
                v: JustifyV::Middle,
            },
            Z_WORLD_TEXT,
            FontSpec {
                // The bound face, not a hardcoded Friz: an addon's assignment is what draws.
                path: font.as_ref().and_then(|f| f.0.as_deref()),
                height: Some(target_px),
                outline: Outline::None,
                alpha_gradient: None,
                spacing: 0.0,
            },
            // A world overlay with its own seat; the UI grid does not apply.
            crate::ui_text::TextSeat::Exact,
        );
        drop(e);
        let (alpha_text, alpha_shadow) = fade_alpha(cat, elapsed_ms);
        let mut bounds: Option<Rect> = None;
        for q in &mut glyphs {
            // The fade replaces the alpha byte, never multiplies it.
            q.color[3] = f32::from(alpha_text) / 255.0;
            bounds = Some(bounds.map_or(q.rect, |b| b.union(q.rect)));
        }
        // The seat (`0x6c7cc0` tail to `0x509520`): clamp the centre inside the viewport, claim
        // through bucket 1 ([`crate::smart_rect`]), and put the ink h-centred with its bottom at
        // the solved centre. The string is created bottom-justified (`6c8254 push 0x2`, through
        // `0x5cdf70`), so the ink rises above the point the claimed rect brackets.
        if let Some(b) = bounds {
            // Claimed box: 1/G48 taller and 1/G44 wider than the glyphs ([`claimed_box_px`]).
            let claim = claimed_box_px(b.width(), size_value, viewport);
            let (hw, hh) = (claim.x * 0.5, claim.y * 0.5);
            let cx = screen
                .x
                .clamp(hw.min(viewport.x - hw), (viewport.x - hw).max(hw));
            let cy = screen
                .y
                .clamp(hh.min(viewport.y - hh), (viewport.y - hh).max(hh));
            let desired = Rect::new(cx - hw, cy - hh, cx + hw, cy + hh);
            let solved = bucket.resolve(desired, viewport);
            bucket.claim(solved);
            let target = (solved.min + solved.max) * 0.5;
            let shift = target - Vec2::new((b.min.x + b.max.x) * 0.5, b.max.y);
            if shift != Vec2::ZERO {
                for q in &mut glyphs {
                    q.rect = Rect {
                        min: q.rect.min + shift,
                        max: q.rect.max + shift,
                    };
                }
            }
        }
        // The shadow (`0x5c27a0`, offset `0xce8804`): black, the fade's shadow lane. Pushed first
        // so the stable z-sort keeps it behind the fill, as `0x5c8710` draws shadow before main.
        if alpha_shadow > 0 {
            let off = shadow_offset_px(viewport);
            quads.overlays.extend(glyphs.iter().map(|q| {
                let mut s = q.clone();
                s.rect = Rect {
                    min: q.rect.min + off,
                    max: q.rect.max + off,
                };
                s.color = [0.0, 0.0, 0.0, f32::from(alpha_shadow) / 255.0];
                s
            }));
        }
        quads.overlays.append(&mut glyphs);
    }
    // Freeing the slot is the point: an unseen number would otherwise hold one of the unit's 4.
    drop_indices(&mut texts.0, culled);
}

/// Remove `idx` (in any order, no duplicates) from `v`, keeping the rest in order.
fn drop_indices<T>(v: &mut Vec<T>, mut idx: Vec<usize>) {
    if idx.is_empty() {
        return;
    }
    idx.sort_unstable();
    let mut i = 0;
    v.retain(|_| {
        let keep = idx.binary_search(&i).is_err();
        i += 1;
        keep
    });
}

/// `CharmedBy`, else `CreatedBy`, when set (`0x4f5f20`).
fn charmer_or_creator(store: &crate::net::ObjectStore) -> Option<u64> {
    let set = |g: Option<u64>| g.filter(|&g| g != 0);
    set(store.0.unit_charmed_by()).or_else(|| set(store.0.unit_created_by()))
}

/// `IsOwnedByActivePlayer` (`0x7190a0`), the source class `K`: the player, or a unit whose charmer
/// or creator, or whose owner's, is the player; `None` floats nothing.
pub(crate) fn owned_by_active_player<'a>(
    source: u64,
    me: Option<u64>,
    store_of: impl Fn(u64) -> Option<&'a crate::net::ObjectStore>,
) -> Option<DamageSource> {
    let me = me?;
    if source == me {
        return Some(DamageSource::Player);
    }
    let owner = store_of(source)
        .filter(|s| s.is_unit())
        .and_then(charmer_or_creator)?;
    let up = store_of(owner).and_then(charmer_or_creator);
    (owner == me || up == Some(me)).then_some(DamageSource::Pet)
}

/// The melee number or word, floated on [`SwingImpact`], the swing's impact keyframe (the
/// reference's `0x6247d0` to `0x624530` deferral), so it lands with the blow. Gate A and `K` apply.
fn melee_impact_text(
    mut impacts: MessageReader<crate::creature_anim::SwingImpact>,
    self_player: Query<(), With<crate::net::SelfPlayer>>,
    self_guid: Res<crate::net::SelfGuid>,
    index: Res<crate::net::GuidIndex>,
    guids: Query<&crate::net::Guid>,
    stores: Query<&crate::net::ObjectStore>,
    gates: Res<DamageTextGates>,
    mut text: MessageWriter<CombatTextSpawn>,
) {
    let store_of = |g: u64| index.0.get(&g).and_then(|&e| stores.get(e).ok());
    for crate::creature_anim::SwingImpact { swing: s, .. } in impacts.read() {
        // `0x62440d`, first in `0x6243e0`: a swing the `0x625e40` verdict hides floats nothing;
        // the flinch, blood and sounds still fire.
        if !s.displayed {
            continue;
        }
        let Some(victim) = s.victim else { continue };
        if self_player.contains(victim) {
            continue; // Gate A: never over your own head
        }
        let Some(source) = guids
            .get(s.attacker)
            .ok()
            .and_then(|g| owned_by_active_player(g.0, self_guid.0, store_of))
        else {
            continue; // K = other: never drawn
        };
        let Some(color) = damage_color(*gates, source, true) else {
            continue; // the CombatDamage and PetMeleeDamage gates
        };
        if let Some((category, body)) = melee_text(s.hit_info, s.victim_state, s.damage) {
            text.write(CombatTextSpawn {
                anchor: victim,
                text: body,
                category,
                color,
            });
        }
    }
}

/// Registers the spawn message, the pool, and the per-frame engine (after the script extract,
/// inside the append window the mesh rebuild waits on).
pub(crate) struct CombatTextPlugin;

/// The floating-text gates' change callback: five flags.
pub(crate) fn on_cvar(ev: On<crate::cvars::CvarChanged>, mut gates: ResMut<DamageTextGates>) {
    match ev.key().as_str() {
        "combatdamage" => gates.combat_damage = ev.flag(),
        "petmeleedamage" => gates.pet_melee = ev.flag(),
        "petspelldamage" => gates.pet_spell = ev.flag(),
        "combatlogperiodicspells" => gates.periodic = ev.flag(),
        "combathealing" => gates.healing = ev.flag(),
        _ => {}
    }
}

impl Plugin for CombatTextPlugin {
    fn build(&self, app: &mut App) {
        // The Update append window ([`UiQuadAppend`]), after the camera controller.
        app.add_observer(on_cvar);
        app.init_resource::<WorldTexts>()
            .init_resource::<DamageTextFont>()
            .init_resource::<DamageTextGates>()
            .add_message::<CombatTextSpawn>()
            .add_systems(
                Update,
                (melee_impact_text, float_combat_text)
                    .chain()
                    .in_set(UiQuadAppend),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::law::COLOR_SPELL_GOLD;
    use super::*;

    /// Headless, and with no attachment on the bare entities the anchor is the feet, so the
    /// snapshot is exactly `z − 1/3`.
    #[test]
    fn cap_is_four_per_unit_hard_drop() {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin);
        app.init_resource::<UiQuads>();
        app.init_resource::<WorldTexts>();
        app.add_message::<CombatTextSpawn>();
        app.add_systems(Update, float_combat_text);
        let unit = app.world_mut().spawn(Transform::default()).id();
        let other = app.world_mut().spawn(Transform::default()).id();
        for i in 0..6 {
            app.world_mut().write_message(CombatTextSpawn {
                anchor: unit,
                text: format!("{i}"),
                category: 0,
                color: None,
            });
        }
        app.world_mut().write_message(CombatTextSpawn {
            anchor: other,
            text: "7".into(),
            category: 0,
            color: Some(COLOR_SPELL_GOLD),
        });
        app.update();
        let texts = app.world().resource::<WorldTexts>();
        assert_eq!(
            texts.0.iter().filter(|t| t.anchor == unit).count(),
            MAX_PER_UNIT,
            "the 5th and 6th spawns are hard-dropped"
        );
        assert_eq!(texts.0.iter().filter(|t| t.anchor == other).count(), 1);
        assert!((texts.0[0].pos.y - (-1.0 / 3.0)).abs() < 1e-6, "z − 1/3");
        // No override takes the row default; an override rides through.
        assert_eq!(texts.0[0].color, 0xFFFF_FFFF);
        let gold = texts.0.iter().find(|t| t.anchor == other).unwrap();
        assert_eq!(gold.color, COLOR_SPELL_GOLD);
    }

    /// The cull's walk collects indices unsorted, against the list before removal.
    #[test]
    fn drop_indices_takes_exactly_those() {
        let mut v = vec!["a", "b", "c", "d", "e"];
        drop_indices(&mut v, vec![3, 0, 1]); // unsorted, as the seat walk produces them
        assert_eq!(v, ["c", "e"]);
        drop_indices(&mut v, vec![]);
        assert_eq!(v, ["c", "e"], "no culls, no change");
        drop_indices(&mut v, vec![1]);
        assert_eq!(v, ["c"]);
        drop_indices(&mut v, vec![0]);
        assert!(v.is_empty());
    }
}
