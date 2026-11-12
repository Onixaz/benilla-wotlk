//! The selection calls of the script call stream ([`crate::script_calls`]), each committed through
//! the one SetSelection path when its turn comes, so a later call in the same pass reads it.

use bevy::prelude::*;

use benilla_ui::script::{SelectionRequest, UiScript};

use super::by_name::{self, ByNameScan, SelectCommit};
use super::scan;

/// Everything the selection calls read and write.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct ScriptSelect<'w, 's> {
    /// The app's unit-token resolver. The VM's own reads the same grammar
    /// ([`benilla_ui::script::parse_unit_token`]), so `TargetUnit("party1target")` and
    /// `CheckInteractDistance("party1target", …)` name the same unit.
    tokens: crate::ui_unit::UnitTokens<'w, 's>,
    /// `TargetLastEnemy`'s memory, stamped by `scan::remember_last_enemy`.
    last_enemy: Res<'w, scan::LastEnemy>,
    /// `TargetLastFriend`'s memory, stamped by `scan::remember_last_friend`.
    last_friend: Res<'w, scan::LastFriend>,
    /// The SetSelection tail shared with the world click, `/target` and `/assist`, classification
    /// included.
    commit: SelectCommit<'w, 's>,
    by_name: ByNameScan<'w, 's>,
    /// The TAB cycler's inputs, for the `TargetNearest*` calls.
    scan: scan::TargetScan<'w, 's>,
    history: ResMut<'w, scan::TabHistory>,
    time: Res<'w, Time>,
    /// `AssistUnit`'s miss lines.
    errors: ResMut<'w, crate::ui_action::UiErrorKeys>,
}

/// `AssistUnit`'s line for a basis it cannot use: an empty or `"target"` token (`0x9fc4a0`) shows
/// `ERR_GENERIC_NO_TARGET`, any other `ERR_UNIT_NOT_FOUND`.
fn assist_miss_key(token: &str) -> &'static str {
    if token.is_empty() || token.eq_ignore_ascii_case("target") {
        "ERR_GENERIC_NO_TARGET"
    } else {
        "ERR_UNIT_NOT_FOUND"
    }
}

impl ScriptSelect<'_, '_> {
    /// `TargetUnit`, `TargetLastEnemy` and `TargetLastFriend` share the reference's helper
    /// (`0x5259e0`): a held unit, else a group member or pet by its guid alone, is committed;
    /// anything else is a no-op, never a deselect. An unheld pick plays its select sound
    /// (`0x524eef`) through `script`.
    pub(crate) fn select(&mut self, request: SelectionRequest, script: &mut UiScript) {
        match request {
            SelectionRequest::Unit(token) => {
                if let Some(guid) = self.tokens.guid_of(&token, &self.commit.selection) {
                    self.target_if_valid(guid, script);
                }
            }
            // The token resolve, then the shared tail. A basis the object manager does not hold
            // shows `ERR_UNIT_NOT_FOUND` (`0x525f87`), or `ERR_GENERIC_NO_TARGET` for an empty or
            // `"target"` token (`0x525f9c`).
            SelectionRequest::Assist(token) => {
                match self.tokens.resolve(&token, &self.commit.selection) {
                    Some((basis, _)) => {
                        if self.commit.assist(basis, "AssistUnit") {
                            script.queue_sound_kit("igCharacterSelect");
                        }
                    }
                    None => {
                        info!("assist (AssistUnit): \"{token}\" names no held unit");
                        self.errors
                            .0
                            .push(crate::ui_action::UiError::key(assist_miss_key(&token)));
                    }
                }
            }
            // `/assist <name>`: a player name, prefix-matched by the app's name scan.
            SelectionRequest::AssistByName(name) => {
                if by_name::assist_named(
                    &self.by_name,
                    &mut self.commit,
                    Some(&name),
                    "AssistByName",
                ) {
                    script.queue_sound_kit("igCharacterSelect");
                }
            }
            // Through the shared helper. Empty memory is a no-op, unlike `TargetLastTarget`, which
            // deselects.
            SelectionRequest::LastFriend => {
                if let Some(guid) = self.last_friend.0 {
                    self.target_if_valid(guid, script);
                }
            }
            // A stale memory is a no-op, and the memory stays, as the reference never clears it.
            SelectionRequest::LastEnemy => match self.last_enemy.0 {
                Some(guid) => self.target_if_valid(guid, script),
                None => info!("TargetLastEnemy: nothing hostile has been targeted yet; no-op"),
            },
        }
    }

    /// The shared helper's commit, with the unheld branch's `igCharacterSelect`.
    fn target_if_valid(&mut self, guid: u64, script: &mut UiScript) {
        if self.commit.target_if_valid(guid) {
            script.queue_sound_kit("igCharacterSelect");
        }
    }

    /// `TargetUnit(name, exact)`, the stock `/target`, with the unheld branch's sound.
    pub(crate) fn target_by_name(&mut self, name: &str, exact: bool, script: &mut UiScript) {
        if by_name::target_named(&self.by_name, &mut self.commit, name, exact) {
            script.queue_sound_kit("igCharacterSelect");
        }
    }

    /// `StartAttack`'s named unit (`0x523090`): a token, else the closest attackable name, as a
    /// unit to commit once the actor checks pass.
    pub(crate) fn attack_unit(&self, unit: &str, exact: bool) -> Option<(Entity, u64)> {
        self.tokens
            .resolve(unit, &self.commit.selection)
            .or_else(|| self.by_name.attackable(unit, exact))
    }

    /// Commit the unit [`Self::attack_unit`] named.
    pub(crate) fn select_for_attack(&mut self, entity: Entity, guid: u64) {
        self.commit.commit(entity, guid);
    }

    /// `TargetNearest*([reverse])`, one cycle of the cycler the TAB key runs, called with
    /// `(reverse, mode)`.
    pub(crate) fn target_nearest(&mut self, mode: benilla_ui::script::NearestMode, reverse: bool) {
        let engaged = self.commit.engaged();
        scan::cycle(
            scan::ScanSide::of(mode),
            reverse,
            self.time.elapsed_secs_f64(),
            &self.scan,
            &mut self.history,
            &mut self.commit.selection,
            &mut self.commit.seam,
            engaged,
        );
    }

    /// `TargetLastTarget()` (`0x525d70`): a last-target pair goes to the shared helper, so a held
    /// unit or a group member is committed (which stamps the outgoing target, swapping the two)
    /// and anything else is a no-op; an empty pair deselects.
    pub(crate) fn target_last_target(&mut self, script: &mut UiScript) {
        match self.commit.selection.last {
            Some(guid) => self.target_if_valid(guid, script),
            None => self.commit.clear(),
        }
    }

    /// `ClearTarget()`: deselect whatever the calls before it left selected.
    pub(crate) fn clear_target(&mut self) {
        self.commit.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A world holding what [`ScriptSelect`] reads, with a bare VM and no units.
    fn script_world() -> World {
        use crate::net::NetCommands;
        use benilla_ui::script::UiScript;

        let (tx, _rx) = crossbeam_channel::unbounded();
        let mut world = World::new();
        world.insert_resource(NetCommands(tx));
        world.init_resource::<crate::spell::QueuedMeleeSpell>();
        world.init_resource::<crate::spell::AutoRepeatActive>();
        world.init_resource::<crate::ui_loot::LootState>();
        world.init_resource::<crate::ui_loot::LootLatch>();
        world.init_resource::<Messages<crate::creature_anim::SheathRequest>>();
        world.init_resource::<crate::ui_party::GroupState>();
        world.init_resource::<crate::net::GuidIndex>();
        world.init_resource::<crate::net::Reputations>();
        world.init_resource::<super::super::AssistAttack>();
        world.init_resource::<super::super::Selection>();
        world.init_resource::<scan::LastEnemy>();
        world.init_resource::<scan::LastFriend>();
        world.init_resource::<crate::names::NameCache>();
        world.init_resource::<scan::TabHistory>();
        world.init_resource::<Time>();
        world.init_resource::<crate::ui_action::UiErrorKeys>();
        world.insert_non_send_resource(UiScript::new().expect("a bare VM"));
        world
    }

    /// The selection calls from Lua, through the binding bodies of ASSISTTARGET (`AssistUnit`) and
    /// TARGETLASTHOSTILE (`TargetLastEnemy`): an empty basis, a garbage token and a stale memory
    /// are each a no-op (the reference's bare `ret`), never a deselect or a panic.
    #[test]
    fn the_selection_calls_run_assist_and_last_enemy_without_ever_deselecting() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const BASIS: u64 = 0xB0A2;
        const VICTIM: u64 = 0xC0DE;
        const GONE: u64 = 0x6017;
        // `UNIT_FIELD_TARGET` is a 2-field guid at index 16; HEALTH/MAXHEALTH keep the units live.
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));

        let mut world = script_world();

        world.spawn((SelfPlayer, Guid(ME), store(&[(22, 100), (28, 100)])));
        // The basis is pointing at the victim; the victim points at nobody.
        let basis = world
            .spawn((
                Guid(BASIS),
                store(&[(22, 100), (28, 100), (16, VICTIM as u32)]),
            ))
            .id();
        let victim = world
            .spawn((Guid(VICTIM), store(&[(22, 100), (28, 100)])))
            .id();
        let index = &mut world.resource_mut::<crate::net::GuidIndex>().0;
        index.insert(BASIS, basis);
        index.insert(VICTIM, victim);

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for request in script.take_selection_requests() {
                            select.select(request, &mut script);
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            world.resource::<super::super::Selection>().guid
        };
        let set = |world: &mut World, target: Option<(Entity, u64)>| {
            let mut sel = world.resource_mut::<super::super::Selection>();
            sel.target = target.map(|(e, _)| e);
            sel.guid = target.map(|(_, g)| g);
        };

        // Assist the current target: one hop onto its target.
        set(&mut world, Some((basis, BASIS)));
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        // The victim targets nobody: assisting it is a no-op, not a deselect.
        assert_eq!(run(&mut world, r#"AssistUnit("target")"#), Some(VICTIM));

        assert_eq!(run(&mut world, r#"AssistUnit("nosuchunit")"#), Some(VICTIM));
        assert_eq!(run(&mut world, r#"TargetUnit("nosuchunit")"#), Some(VICTIM));

        // TargetLastEnemy with nothing remembered: also a no-op.
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        world.resource_mut::<scan::LastEnemy>().0 = Some(VICTIM);
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));

        // A stale memory (the unit despawned, so its guid is in no index) leaves the target alone.
        world.resource_mut::<scan::LastEnemy>().0 = Some(GONE);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), Some(VICTIM));
        set(&mut world, None);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), None);
    }

    /// `TargetUnit("partypet1")`, the TARGETPARTYPET1 binding and the pet half of
    /// TARGETPARTYMEMBER1, commits the pet the member's descriptor names; a slot with no member, or
    /// one whose pet is not held, is a no-op, never a deselect.
    #[test]
    fn target_unit_selects_a_party_pet() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const MEMBER: u64 = 0x1234;
        const PET: u64 = 0xF140_0000_0000_0077;
        // HEALTH/MAXHEALTH keep the units live; `UNIT_FIELD_SUMMON` is the 2-field guid at 8.
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));

        let mut world = script_world();
        world.spawn((SelfPlayer, Guid(ME), store(&[(22, 100), (28, 100)])));
        let pet = world
            .spawn((Guid(PET), store(&[(22, 100), (28, 100)])))
            .id();
        let member = world
            .spawn((
                Guid(MEMBER),
                store(&[
                    (22, 100),
                    (28, 100),
                    (8, PET as u32),
                    (9, (PET >> 32) as u32),
                ]),
            ))
            .id();
        let index = &mut world.resource_mut::<crate::net::GuidIndex>().0;
        index.insert(PET, pet);
        index.insert(MEMBER, member);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    status: member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for request in script.take_selection_requests() {
                            select.select(request, &mut script);
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            world.resource::<super::super::Selection>().guid
        };

        assert_eq!(run(&mut world, r#"TargetUnit("party1")"#), Some(MEMBER));
        assert_eq!(run(&mut world, r#"TargetUnit("partypet1")"#), Some(PET));
        assert_eq!(run(&mut world, r#"TargetUnit("PartyPet1")"#), Some(PET));
        // No second member, and slot 0 and 5 are no slot: each leaves the selection alone.
        for token in ["partypet2", "partypet0", "partypet5"] {
            assert_eq!(
                run(&mut world, &format!(r#"TargetUnit("{token}")"#)),
                Some(PET),
                "{token}"
            );
        }
        // Out of view, the pet is named by its held owner's descriptor: selected by guid alone
        // (`0x5259e0`'s group leg).
        world.resource_mut::<crate::net::GuidIndex>().0.remove(&PET);
        assert_eq!(run(&mut world, r#"TargetUnit("party1")"#), Some(MEMBER));
        assert_eq!(run(&mut world, r#"TargetUnit("partypet1")"#), Some(PET));
        assert_eq!(world.resource::<super::super::Selection>().target, None);
    }

    /// The stock party frame, off the player's own 3.3.5a chain: `PartyMemberFrame1` is a secure
    /// unit button on `party1` (`PartyMemberFrame.lua`'s `SecureUnitButton_OnLoad`), so a left
    /// click on a member the object manager does not hold queues `TargetUnit("party1")`, which
    /// selects the roster guid.
    #[test]
    fn clicking_an_unstreamed_members_stock_party_frame_selects_them() {
        benilla_formats::wow_data_or_skip!();
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_ui::script::{PartyMemberInfo, PartyState, ScriptValue, UiScript, UnitState};
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const MEMBER: u64 = 0x1234;
        let mut world = script_world();
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    status: member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );
        {
            let mut s = world.non_send_resource_mut::<UiScript>();
            s.set_screen_size(1024.0, 768.0);
            crate::ui_script::test_ui::load_stock_ui(&mut s);
            s.set_party(PartyState {
                members: vec![PartyMemberInfo {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    online: true,
                }],
                ..PartyState::default()
            });
            // The feed's unstreamed leg: the roster record, no object.
            s.set_unit(
                "party1",
                Some(UnitState {
                    exists: true,
                    guid: MEMBER,
                    name: Some("Brisca".into()),
                    is_player: true,
                    is_connected: true,
                    health: 50,
                    max_health: 100,
                    level: 80,
                    reaction: 5,
                    ..UnitState::default()
                }),
            );
            s.fire_event("PARTY_MEMBERS_CHANGED", vec![]);
            s.fire_event("UNIT_HEALTH", vec![ScriptValue::Str("party1".into())]);
            s.run(r#"PartyMemberFrame1:Click("LeftButton")"#).unwrap();
        }
        world
            .run_system_once(
                |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                    for call in script.take_script_calls() {
                        if let benilla_ui::script::ScriptCall::Select(request) = call {
                            select.select(request, &mut script);
                        }
                    }
                },
            )
            .expect("the applier runs as a one-shot system");
        let sel = world.resource::<super::super::Selection>();
        assert_eq!((sel.target, sel.guid), (None, Some(MEMBER)));
    }

    /// The issue's case: party1 is in another zone, so the object manager holds no unit, and
    /// `TargetUnit("party1")` selects the roster guid alone, with the unheld branch's sound
    /// (`0x524eef`). A stranger's guid out of view, a garbage token and an empty slot stay no-ops;
    /// `TargetLastTarget` swaps back to the unheld member through the same helper.
    #[test]
    fn target_unit_selects_an_unstreamed_party_member_by_guid() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_ui::script::{SoundRequest, UiScript};
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const MEMBER: u64 = 0x1234;
        const MOB: u64 = 0xF130_0000_0000_0042;
        const STRANGER: u64 = 0x5555;
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));

        let mut world = script_world();
        world.spawn((SelfPlayer, Guid(ME), store(&[(22, 100), (28, 100)])));
        let mob = world
            .spawn((Guid(MOB), store(&[(22, 100), (28, 100)])))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(MOB, mob);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    status: member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for call in script.take_script_calls() {
                            match call {
                                benilla_ui::script::ScriptCall::Select(request) => {
                                    select.select(request, &mut script);
                                }
                                benilla_ui::script::ScriptCall::TargetLastTarget => {
                                    select.target_last_target(&mut script);
                                }
                                _ => {}
                            }
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            let sel = world.resource::<super::super::Selection>();
            (sel.target, sel.guid)
        };
        let sounds = |world: &mut World| world.non_send_resource_mut::<UiScript>().take_sounds();
        let select_sound = vec![SoundRequest::KitName("igCharacterSelect".into())];

        assert_eq!(
            run(&mut world, r#"TargetUnit("party1")"#),
            (None, Some(MEMBER))
        );
        assert_eq!(sounds(&mut world), select_sound);
        // The setter's dedup: no second sound.
        assert_eq!(
            run(&mut world, r#"TargetUnit("party1")"#),
            (None, Some(MEMBER))
        );
        assert!(sounds(&mut world).is_empty());

        // A held unit, then back through the last-target pair.
        world.resource_mut::<scan::LastEnemy>().0 = Some(MOB);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), (Some(mob), Some(MOB)));
        assert!(
            sounds(&mut world).is_empty(),
            "the held branch is silent here"
        );
        assert_eq!(run(&mut world, "TargetLastTarget()"), (None, Some(MEMBER)));
        assert_eq!(sounds(&mut world), select_sound);

        // No-ops, never a deselect: an empty slot, an unheld chain, a stranger out of view.
        for lua in [
            r#"TargetUnit("party2")"#,
            r#"TargetUnit("party1target")"#,
            r#"TargetUnit("nosuchunit")"#,
        ] {
            assert_eq!(run(&mut world, lua), (None, Some(MEMBER)), "{lua}");
        }
        world.resource_mut::<scan::LastEnemy>().0 = Some(STRANGER);
        assert_eq!(run(&mut world, "TargetLastEnemy()"), (None, Some(MEMBER)));
        assert!(sounds(&mut world).is_empty());
    }

    /// `AssistUnit` hands the basis's target to the same helper (`0x525f25`): a group member the
    /// object manager does not hold is selected by guid alone, with the unheld sound, and a
    /// stranger out of view is a no-op. A basis it does not hold shows its miss line: the target
    /// token `ERR_GENERIC_NO_TARGET`, any other `ERR_UNIT_NOT_FOUND`.
    #[test]
    fn assist_unit_selects_an_unheld_member_and_names_a_missing_basis() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_ui::script::{SoundRequest, UiScript};
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const BASIS: u64 = 0xF130_0000_0000_0042;
        const MEMBER: u64 = 0x1234;
        const STRANGER: u64 = 0x5555;
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));

        let mut world = script_world();
        world.spawn((SelfPlayer, Guid(ME), store(&[(22, 100), (28, 100)])));
        let basis = world
            .spawn((
                Guid(BASIS),
                store(&[(22, 100), (28, 100), (16, MEMBER as u32)]),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(BASIS, basis);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    status: member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );
        let select = |world: &mut World, target: Option<(Entity, u64)>| {
            let mut sel = world.resource_mut::<super::super::Selection>();
            sel.target = target.map(|(e, _)| e);
            sel.guid = target.map(|(_, g)| g);
        };
        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for request in script.take_selection_requests() {
                            select.select(request, &mut script);
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            let sel = world.resource::<super::super::Selection>();
            (sel.target, sel.guid)
        };
        let sounds = |world: &mut World| world.non_send_resource_mut::<UiScript>().take_sounds();
        let errors = |world: &mut World| {
            std::mem::take(&mut world.resource_mut::<crate::ui_action::UiErrorKeys>().0)
        };

        select(&mut world, Some((basis, BASIS)));
        assert_eq!(
            run(&mut world, r#"AssistUnit("target")"#),
            (None, Some(MEMBER))
        );
        assert_eq!(
            sounds(&mut world),
            vec![SoundRequest::KitName("igCharacterSelect".into())]
        );
        assert!(errors(&mut world).is_empty());

        // The member is the target now, held by guid alone: the basis is gone.
        assert_eq!(
            run(&mut world, r#"AssistUnit("target")"#),
            (None, Some(MEMBER))
        );
        assert_eq!(
            errors(&mut world),
            vec![crate::ui_action::UiError::key("ERR_GENERIC_NO_TARGET")]
        );
        for lua in [r#"AssistUnit("party1")"#, r#"AssistUnit("nosuchunit")"#] {
            assert_eq!(run(&mut world, lua), (None, Some(MEMBER)), "{lua}");
            assert_eq!(
                errors(&mut world),
                vec![crate::ui_action::UiError::key("ERR_UNIT_NOT_FOUND")],
                "{lua}"
            );
        }
        assert_eq!(run(&mut world, "AssistUnit()"), (None, Some(MEMBER)));
        assert_eq!(
            errors(&mut world),
            vec![crate::ui_action::UiError::key("ERR_GENERIC_NO_TARGET")]
        );

        // A stranger out of view: a no-op, silent.
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .remove(&BASIS);
        world.despawn(basis);
        let basis = world
            .spawn((
                Guid(BASIS),
                store(&[(22, 100), (28, 100), (16, STRANGER as u32)]),
            ))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(BASIS, basis);
        select(&mut world, Some((basis, BASIS)));
        assert_eq!(
            run(&mut world, r#"AssistUnit("target")"#),
            (Some(basis), Some(BASIS))
        );
        assert!(sounds(&mut world).is_empty());
        assert!(errors(&mut world).is_empty());
    }

    /// `TargetUnit` and `AssistUnit` take the tokens the reference's resolver takes: a base then a
    /// hop off each held unit's target, so `party1target` names the unit party1 targets and
    /// `AssistUnit("party1target")` its target's. A chain that breaks, and text that is not a hop,
    /// is a no-op, never a deselect.
    #[test]
    fn target_and_assist_unit_follow_a_target_chain() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const MEMBER: u64 = 0x1234;
        const PET: u64 = 0xF140_0000_0000_0077;
        const VICTIM: u64 = 0xC0DE;
        const THIRD: u64 = 0xF130_0000_0000_0003;
        // `OBJECT_FIELD_TYPE` 2 (a hop is off a unit, typemask 8), `UNIT_FIELD_SUMMON` 8 and
        // `UNIT_FIELD_TARGET` 16, the latter two each a 2-field guid.
        let unit = |kind: u32, summon: u64, target: u64| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, kind),
                (8, summon as u32),
                (9, (summon >> 32) as u32),
                (16, target as u32),
                (17, (target >> 32) as u32),
            ]))
        };
        const PLAYER: u32 = 0x19;
        const CREATURE: u32 = 0x09;

        let mut world = script_world();
        // We target the victim, party1 targets the victim, the victim targets `THIRD`, our pet
        // targets `THIRD`; `THIRD` targets nobody.
        world.spawn((SelfPlayer, Guid(ME), unit(PLAYER, PET, VICTIM)));
        let member = world.spawn((Guid(MEMBER), unit(PLAYER, 0, VICTIM))).id();
        let victim = world.spawn((Guid(VICTIM), unit(CREATURE, 0, THIRD))).id();
        let third = world.spawn((Guid(THIRD), unit(CREATURE, 0, 0))).id();
        let pet = world.spawn((Guid(PET), unit(CREATURE, 0, THIRD))).id();
        let index = &mut world.resource_mut::<crate::net::GuidIndex>().0;
        index.insert(MEMBER, member);
        index.insert(VICTIM, victim);
        index.insert(THIRD, third);
        index.insert(PET, pet);
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![GroupMemberEntry {
                    name: "Brisca".into(),
                    guid: MEMBER,
                    status: member_status::ONLINE,
                    flags: 0,
                    roles: 0,
                }],
                ME,
                None,
                Some(ME),
            );
        let mut bar = crate::ui_pet::PetBar::default();
        bar.spells.pet_guid = PET;
        world.insert_resource(bar);

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for request in script.take_selection_requests() {
                            select.select(request, &mut script);
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            world.resource::<super::super::Selection>().guid
        };
        let reset = |world: &mut World| {
            let mut sel = world.resource_mut::<super::super::Selection>();
            sel.target = None;
            sel.guid = None;
        };

        assert_eq!(run(&mut world, r#"TargetUnit("party1")"#), Some(MEMBER));
        assert_eq!(
            run(&mut world, r#"TargetUnit("party1target")"#),
            Some(VICTIM)
        );
        assert_eq!(
            run(&mut world, r#"TargetUnit("party1targettarget")"#),
            Some(THIRD)
        );
        // `THIRD` targets nobody: the chain ends there, and the selection stays.
        for token in [
            "party1targettargettarget",
            "party1targetfoo",
            "party1foo",
            "party2target",
            "npctarget",
        ] {
            assert_eq!(
                run(&mut world, &format!(r#"TargetUnit("{token}")"#)),
                Some(THIRD),
                "{token}"
            );
        }

        reset(&mut world);
        assert_eq!(
            run(&mut world, r#"TargetUnit("PARTY1TARGET")"#),
            Some(VICTIM)
        );
        reset(&mut world);
        assert_eq!(
            run(&mut world, r#"TargetUnit("playertarget")"#),
            Some(VICTIM)
        );
        reset(&mut world);
        assert_eq!(run(&mut world, r#"TargetUnit("pettarget")"#), Some(THIRD));
        reset(&mut world);
        assert_eq!(run(&mut world, r#"TargetUnit("partypet1target")"#), None);

        // Assisting a unit selects its target; its target is a chain's end, so one hop further.
        reset(&mut world);
        assert_eq!(run(&mut world, r#"AssistUnit("party1")"#), Some(VICTIM));
        reset(&mut world);
        assert_eq!(
            run(&mut world, r#"AssistUnit("party1target")"#),
            Some(THIRD)
        );
        assert_eq!(
            run(&mut world, r#"AssistUnit("party1targettarget")"#),
            Some(THIRD),
            "THIRD targets nobody: silent, nothing deselected"
        );
    }

    /// `TargetUnit("npc")` and `AssistUnit("npc")` name the interaction NPC, which the reference
    /// reads on a whole-token `npc` compare: the vendor whose window is open, and for an assist the
    /// unit it targets. With no window open, or the NPC no longer held, `npc` names nobody and the
    /// selection stays.
    #[test]
    fn target_and_assist_unit_name_the_interaction_npc() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use crate::ui_session::InteractNpc;
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const VENDOR: u64 = 0xF130_0000_9800_0001;
        const GUARD: u64 = 0xF130_0000_0000_0002;
        // `OBJECT_FIELD_TYPE` 2 and `UNIT_FIELD_TARGET` 16, a 2-field guid.
        let unit = |target: u64| {
            ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
                (2, 0x09),
                (16, target as u32),
                (17, (target >> 32) as u32),
            ]))
        };

        let mut world = script_world();
        world.spawn((SelfPlayer, Guid(ME), unit(0)));
        let vendor = world.spawn((Guid(VENDOR), unit(GUARD))).id();
        let guard = world.spawn((Guid(GUARD), unit(0))).id();
        let index = &mut world.resource_mut::<crate::net::GuidIndex>().0;
        index.insert(VENDOR, vendor);
        index.insert(GUARD, guard);
        world.insert_resource(InteractNpc(Some(vendor), Some(VENDOR)));

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for request in script.take_selection_requests() {
                            select.select(request, &mut script);
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            world.resource::<super::super::Selection>().guid
        };
        let reset = |world: &mut World| {
            let mut sel = world.resource_mut::<super::super::Selection>();
            sel.target = None;
            sel.guid = None;
        };

        assert_eq!(run(&mut world, r#"TargetUnit("npc")"#), Some(VENDOR));
        reset(&mut world);
        assert_eq!(run(&mut world, r#"TargetUnit("NPC")"#), Some(VENDOR));
        reset(&mut world);
        assert_eq!(run(&mut world, r#"AssistUnit("npc")"#), Some(GUARD));

        // The NPC streamed out with its window still up: the object manager holds no unit.
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .remove(&VENDOR);
        assert_eq!(run(&mut world, r#"TargetUnit("npc")"#), Some(GUARD));

        // The window closed: nobody, and the selection stays.
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        world.insert_resource(InteractNpc::default());
        assert_eq!(run(&mut world, r#"TargetUnit("npc")"#), Some(GUARD));
    }

    /// The path a player takes: `/script TargetUnit("npc")` typed into the stock chat box runs
    /// `SlashCmdList["SCRIPT"]` (`ChatFrame.lua:1074-1075`) and selects the vendor whose window is
    /// open.
    #[test]
    #[ignore = "reads 1.12.1 data; port to the 3.3.5a install (#51)"]
    fn typing_target_unit_npc_in_the_stock_chat_box_selects_the_vendor() {
        benilla_formats::wow_data_or_skip!();
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use crate::ui_session::InteractNpc;
        use benilla_ui::script::UiScript;
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const VENDOR: u64 = 0xF130_0000_9800_0001;
        let unit = || ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[(2, 0x09)]));

        let mut world = script_world();
        world.insert_non_send_resource(crate::ui_script::chat_tests::chat_frame());
        world.spawn((SelfPlayer, Guid(ME), unit()));
        let vendor = world.spawn((Guid(VENDOR), unit())).id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(VENDOR, vendor);
        world.insert_resource(InteractNpc(Some(vendor), Some(VENDOR)));

        let mut script = world.non_send_resource_mut::<UiScript>();
        assert!(script.focus_editbox("ChatFrameEditBox"));
        script.char_input(r#"/script TargetUnit("npc")"#);
        assert!(script.key_input("ENTER"), "the box consumes ENTER");
        world
            .run_system_once(
                |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                    for request in script.take_selection_requests() {
                        select.select(request, &mut script);
                    }
                },
            )
            .expect("the applier runs as a one-shot system");
        assert_eq!(
            world.resource::<super::super::Selection>().guid,
            Some(VENDOR)
        );
    }

    /// `TargetUnit("<name>")` and the chat `/target <name>` reach a party member out of view
    /// (`0x5202e1`): by the whole name, by a prefix, behind a visible match and ahead of ourselves,
    /// with the unheld branch's sound; never with the exact flag on a prefix, a member with no name
    /// record, or `StartAttack`'s name.
    #[test]
    fn target_by_name_selects_a_party_member_out_of_view() {
        use crate::net::{Guid, ObjectStore, SelfPlayer};
        use benilla_protocol::messages::{member_status, GroupMemberEntry};
        use benilla_ui::script::{SoundRequest, UiScript};
        use bevy::ecs::system::RunSystemOnce;

        const ME: u64 = 1;
        const BRISCA: u64 = 0x1234;
        const QUILL: u64 = 0x1235;
        const CLARA: u64 = 0x1236;
        const NEAR: u64 = 0x5555;
        let store =
            |pairs: &[(u16, u32)]| ObjectStore(benilla_protocol::ObjectFields::from_pairs(pairs));
        let live = || store(&[(22, 100), (28, 100)]);

        let mut world = script_world();
        world.init_resource::<Messages<super::super::by_name::TargetByNameRequest>>();
        let me = world
            .spawn((SelfPlayer, Guid(ME), Transform::default(), live()))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(ME, me);
        let member = |name: &str, guid| GroupMemberEntry {
            name: name.into(),
            guid,
            status: member_status::ONLINE,
            flags: 0,
            roles: 0,
        };
        world
            .resource_mut::<crate::ui_party::GroupState>()
            .apply_list(
                0,
                0,
                vec![
                    member("Brisca", BRISCA),
                    member("Quill", QUILL),
                    member("Clara", CLARA),
                ],
                ME,
                None,
                Some(ME),
            );
        {
            // Quill has no name record.
            let mut names = world.resource_mut::<crate::names::NameCache>();
            for (guid, name) in [
                (ME, "Claude"),
                (BRISCA, "Brisca"),
                (CLARA, "Clara"),
                (NEAR, "Brismond"),
            ] {
                names.insert_player(guid, name.into(), None);
            }
        }

        let run = |world: &mut World, lua: &str| {
            world
                .non_send_resource_mut::<UiScript>()
                .eval::<()>(lua)
                .expect("the binding runs");
            world
                .run_system_once(
                    |mut script: NonSendMut<UiScript>, mut select: ScriptSelect| {
                        for call in script.take_script_calls() {
                            match call {
                                benilla_ui::script::ScriptCall::TargetByName { name, exact } => {
                                    select.target_by_name(&name, exact, &mut script);
                                }
                                benilla_ui::script::ScriptCall::ClearTarget => {
                                    select.clear_target();
                                }
                                _ => {}
                            }
                        }
                    },
                )
                .expect("the applier runs as a one-shot system");
            let sel = world.resource::<super::super::Selection>();
            (sel.target, sel.guid)
        };
        let sounds = |world: &mut World| world.non_send_resource_mut::<UiScript>().take_sounds();
        let select_sound = vec![SoundRequest::KitName("igCharacterSelect".into())];

        assert_eq!(
            run(&mut world, r#"TargetUnit("Brisca")"#),
            (None, Some(BRISCA))
        );
        assert_eq!(sounds(&mut world), select_sound);
        assert_eq!(
            run(&mut world, r#"ClearTarget() TargetUnit("bri")"#),
            (None, Some(BRISCA)),
            "a prefix of the member's name"
        );
        assert_eq!(sounds(&mut world), select_sound);
        assert_eq!(
            run(&mut world, r#"ClearTarget() TargetUnit("bri", 1)"#),
            (None, None),
            "the exact flag compares the whole name"
        );
        assert_eq!(
            run(&mut world, r#"TargetUnit("Quill")"#),
            (None, None),
            "no name record, no match"
        );
        assert_eq!(
            run(&mut world, r#"TargetUnit("Cla")"#),
            (None, Some(CLARA)),
            "we scan at the group's distance, behind the member"
        );
        assert_eq!(sounds(&mut world), select_sound);
        assert_eq!(
            run(&mut world, r#"TargetUnit("Claude")"#),
            (Some(me), Some(ME)),
            "our own whole name"
        );
        assert_eq!(
            run(&mut world, r#"TargetUnit("Clau")"#),
            (Some(me), Some(ME)),
            "ourselves, when no member matches"
        );

        // A visible match outbids the member out of view; the member's whole name still wins.
        let near = world
            .spawn((Guid(NEAR), Transform::from_xyz(10.0, 0.0, 0.0), live()))
            .id();
        world
            .resource_mut::<crate::net::GuidIndex>()
            .0
            .insert(NEAR, near);
        sounds(&mut world);
        assert_eq!(
            run(&mut world, r#"TargetUnit("Bri")"#),
            (Some(near), Some(NEAR))
        );
        assert!(
            sounds(&mut world).is_empty(),
            "the held branch is silent here"
        );
        assert_eq!(
            run(&mut world, r#"TargetUnit("brisca")"#),
            (None, Some(BRISCA))
        );

        // `StartAttack`'s name (filter mode 1) skips the group scans.
        let attack = world
            .run_system_once(|select: ScriptSelect| select.attack_unit("Clara", false))
            .expect("the resolve runs as a one-shot system");
        assert_eq!(attack, None);

        // The chat `/target`, with its sound off the VM.
        sounds(&mut world);
        world.write_message(super::super::by_name::TargetByNameRequest {
            name: "clar".into(),
        });
        world
            .run_system_once(super::super::by_name::target_by_name_requests)
            .expect("the chat drain runs as a one-shot system");
        assert_eq!(
            world.resource::<super::super::Selection>().guid,
            Some(CLARA)
        );
        assert_eq!(sounds(&mut world), select_sound);
    }
}
