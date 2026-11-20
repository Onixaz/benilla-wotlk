//! The flush behind every mutation of the pet's cooldown list, whole client headless: each packet that reaches the list arrives as the session event it is and flushes in its handler, each press in its applier. The reference fires `ACTIONBAR_UPDATE_COOLDOWN` then `SPELL_UPDATE_COOLDOWN`, and then `PET_BAR_UPDATE_COOLDOWN`; none of the callers fires `PET_BAR_UPDATE`.

use std::time::Duration;

use benilla_formats::SpellDisplay;
use benilla_protocol::messages::{PetSpellCooldown, PetSpells};
use benilla_protocol::SessionEvent;

use super::press_tests::{claw, growl, listen, listening_vm, rig, Rig, CLAW, GROWL, ME, PET};
use super::PetBar;
use crate::net::ObjectStore;

/// The three events, in the reference's order.
const FLUSH: [&str; 3] = [
    "ACTIONBAR_UPDATE_COOLDOWN",
    "SPELL_UPDATE_COOLDOWN",
    "PET_BAR_UPDATE_COOLDOWN",
];

/// `n` flushes back to back, one per mutation.
fn flushes(n: usize) -> Vec<&'static str> {
    FLUSH.repeat(n)
}

/// `UNIT_FIELD_SUMMONEDBY`, low word, the owner test the GO's pet leg reads.
const SUMMONEDBY: u16 = benilla_protocol::field::FIELD_UNIT_SUMMONEDBY;

/// Our own pet by the field the GO's pet leg reads: `SUMMONEDBY` us.
fn summoned_pet() -> ObjectStore {
    ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (SUMMONEDBY, ME as u32),
        (SUMMONEDBY + 1, 0),
    ]))
}

/// Claw as the pet book lists it: named, iconed, and with the ordinary pet GCD.
fn claw_book_spell() -> SpellDisplay {
    SpellDisplay {
        icon: Some("Interface\\Icons\\Ability_Druid_Rake".into()),
        ..claw()
    }
}

/// A rig with Growl (a 5 s recovery of its own) on slot 1 of `PET`'s bar.
fn growl_rig() -> Rig {
    rig(vec![(GROWL, growl())], &[GROWL], Some(summoned_pet()))
}

/// `SMSG_SPELL_GO` of `spell_id` by `caster`, as the session decodes a plain cast.
fn go(caster: u64, spell_id: u32) -> SessionEvent {
    SessionEvent::SpellGo {
        caster,
        cast_count: 0,
        spell_id,
        cast_flags: 0,
        server_time: 0,
        hits: vec![],
        misses: vec![],
        reflects: vec![],
        target: None,
        go_target: None,
        target_mask: 0,
        src: None,
        dest: None,
        ammo: None,
        item_caster: None,
        trajectory: None,
        dest_counter: 0,
        points: vec![],
    }
}

impl Rig {
    /// The packets, handled as the session hands them to the client.
    fn receive(&mut self, events: Vec<SessionEvent>) {
        crate::net::handlers::dispatch(self.app.world_mut(), events);
    }

    fn generation(&self) -> u64 {
        self.app.world().resource::<PetBar>().cooldowns.generation
    }

    /// How many records the pet's list holds, elapsed ones included until a collection.
    fn record_count(&self) -> usize {
        self.app
            .world()
            .resource::<PetBar>()
            .cooldowns
            .record_count()
    }
}

/// The GO's pet leg inserts into the pet's list, then flushes. The bar takes no `PET_BAR_UPDATE`
/// from it, which the spellbook's pet tab would take as a repaint of its own.
#[test]
fn a_pets_spell_go_fires_the_flush_and_no_bar_update() {
    let mut rig = growl_rig();

    rig.receive(vec![go(PET, GROWL)]);

    assert_eq!(rig.frame(), FLUSH);
    assert!(rig.frame().is_empty(), "once, not every frame");
}

/// The GO's owner test admits our pet and nobody else's: no insert, no flush.
#[test]
fn a_strangers_pet_go_flushes_nothing() {
    let strangers = ObjectStore(benilla_protocol::ObjectFields::from_pairs(&[
        (SUMMONEDBY, 0x99),
        (SUMMONEDBY + 1, 0),
    ]));
    let mut rig = rig(vec![(GROWL, growl())], &[GROWL], Some(strangers));

    rig.receive(vec![go(PET, GROWL)]);

    assert_eq!(rig.generation(), 0);
    assert!(rig.frame().is_empty());
}

/// `0x807033`, `0x807039`: the pet bar's flush follows `SMSG_SPELL_COOLDOWN` for the pet's guid,
/// and for it alone; the player's own list signals the spellbook's pair and the bags' instead
/// (`0x80702e`, `0x807048`).
#[test]
fn a_pet_addressed_spell_cooldown_fires_the_flush() {
    let mut rig = growl_rig();

    rig.receive(vec![SessionEvent::SpellCooldowns {
        caster: ME,
        flags: 0,
        cooldowns: vec![(GROWL, 3000)],
    }]);
    assert_eq!(
        rig.frame(),
        ["ACTIONBAR_UPDATE_COOLDOWN", "SPELL_UPDATE_COOLDOWN"],
        "the player's list is not the pet's flush"
    );

    rig.receive(vec![SessionEvent::SpellCooldowns {
        caster: PET,
        flags: 0,
        cooldowns: vec![(GROWL, 3000)],
    }]);
    assert_eq!(rig.frame(), FLUSH);
}

/// (`SMSG_COOLDOWN_EVENT`, `SMSG_CLEAR_COOLDOWN`) and (`SMSG_COOLDOWN_CHEAT`): each flushes for the
/// pet's list as for a spell cast.
#[test]
fn the_pets_event_clear_and_cheat_packets_fire_the_flush() {
    // Attributes bit 25: the record parks until the event.
    let parked = || SpellDisplay {
        attributes: 1 << 25,
        ..growl()
    };
    let mut rig = rig(vec![(GROWL, parked())], &[GROWL], Some(summoned_pet()));
    let park = |rig: &mut Rig| {
        rig.receive(vec![SessionEvent::SpellCooldowns {
            caster: PET,
            flags: 0,
            cooldowns: vec![(GROWL, 3000)],
        }]);
        assert_eq!(rig.frame(), FLUSH, "parked");
    };

    park(&mut rig);
    rig.receive(vec![SessionEvent::CooldownEvent {
        spell_id: GROWL,
        caster: PET,
    }]);
    assert_eq!(rig.frame(), FLUSH, "the parked timers start");

    rig.receive(vec![SessionEvent::ClearCooldown {
        spell_id: GROWL,
        caster: PET,
    }]);
    assert_eq!(rig.frame(), FLUSH, "the record goes");

    park(&mut rig);
    rig.receive(vec![SessionEvent::CooldownCheat { caster: PET }]);
    assert_eq!(rig.frame(), FLUSH, "the GM wipe");
}

/// Each packet flushes in its own handler, two in one frame included (`0x80fe4b`, `0x807039`).
#[test]
fn each_packet_of_a_frame_fires_its_own_flush() {
    let mut rig = growl_rig();

    rig.receive(vec![
        go(PET, GROWL),
        SessionEvent::SpellCooldowns {
            caster: PET,
            flags: 0,
            cooldowns: vec![(GROWL, 3000)],
        },
    ]);

    assert_eq!(rig.frame(), flushes(2));
}

/// `SMSG_PET_SPELLS` seeds the list, which flushes nothing, and ends in the pet setter's
/// `PET_BAR_UPDATE` signal (event `0x161`) whatever the packet changed: a packet that moves only
/// the cooldowns is one `PET_BAR_UPDATE`, not the flush.
#[test]
fn the_pet_spells_packet_signals_the_bar_and_never_flushes() {
    let mut rig = growl_rig();
    let same_bar = PetSpells {
        cooldowns: vec![PetSpellCooldown {
            spell_id: GROWL,
            category: 82,
            spell_cd_ms: 4000,
            category_cd_ms: 0,
        }],
        ..rig.app.world().resource::<PetBar>().spells.clone()
    };

    rig.receive(vec![SessionEvent::PetSpells(Box::new(same_bar))]);
    assert_eq!(
        rig.frame(),
        ["PET_BAR_UPDATE"],
        "the bar is what it was and its cooldown is new: the packet's own signal repaints it"
    );
    let (_, duration, enable) = rig.cooldown(1);
    assert!(
        (duration - 4.0).abs() < 1e-9 && enable == 1,
        "and the seeded cooldown reached the bar: {duration}"
    );
    assert_eq!(rig.generation(), 0, "a seed is no mutation of the flush");

    // The teardown clears the list and flushes nothing either.
    rig.receive(vec![SessionEvent::PetSpells(Box::default())]);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"]);
}

/// A reset moves no counter back: a mutation, the teardown, a new bar, and the next mutation
/// flushes once. A counter that fell back to 0 would read as a mutation at the teardown.
#[test]
fn a_teardown_and_a_new_bar_do_not_hide_the_next_flush() {
    let mut rig = growl_rig();
    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);
    let held = rig.generation();

    rig.receive(vec![SessionEvent::PetSpells(Box::default())]);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the teardown");
    assert_eq!(rig.generation(), held, "the counter is where it was");

    let mut bar = rig.app.world().resource::<PetBar>().spells.clone();
    bar.pet_guid = PET;
    bar.bar[0] = benilla_protocol::messages::PetActionEntry::from(
        GROWL | (u32::from(benilla_protocol::messages::PET_ACT_ENABLED) << 24),
    );
    rig.receive(vec![SessionEvent::PetSpells(Box::new(bar))]);
    assert_eq!(rig.frame(), ["PET_BAR_UPDATE"], "the new bar");

    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);
}

/// The stock `Cooldown` frame ends its own sweep, and the reference flushes no event for the end
/// of a cooldown: an elapsed record moves neither the list nor the events.
#[test]
fn a_natural_expiry_fires_nothing() {
    let brief = || SpellDisplay {
        name: "Brief".into(),
        recovery_ms: 30,
        ..Default::default()
    };
    let mut rig = rig(vec![(GROWL, brief())], &[GROWL], Some(summoned_pet()));
    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);
    assert!(rig.cooldown(1).1 > 0.0, "running");

    std::thread::sleep(Duration::from_millis(80));

    assert!(
        rig.frame().is_empty(),
        "its end is the frame's, not an event"
    );
    assert_eq!(
        rig.cooldown(1).1,
        0.0,
        "and the sweep is gone from the push"
    );
}

/// A new VM meets the list as it stands: the flush is for a mutation, not for a load, and the bar's verbs read the list at the call, so nothing is pushed or fired for it.
#[test]
fn a_new_vm_is_not_flushed_for_a_list_that_already_moved() {
    let mut rig = growl_rig();
    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH);

    // The interface reloads: a new VM, which reads the running cooldown at its first call.
    rig.app.world_mut().insert_non_send_resource(listening_vm());
    assert!(rig.frame().is_empty(), "no flush and no bar update");
    assert_eq!(rig.cooldown(1).1, 5.0, "Growl's 5 s, read at the call");

    rig.receive(vec![go(PET, GROWL)]);
    assert_eq!(rig.frame(), FLUSH, "the next mutation is a flush");
}

/// The stock spellbook (`SpellBookFrame.lua:209`-`231`): a `SpellButton` redraws on
/// `SPELL_UPDATE_COOLDOWN`, and on `PET_BAR_UPDATE` while the pet tab is open, and never hears
/// `PET_BAR_UPDATE_COOLDOWN`. So the GCD a pet-bar press starts sweeps the pet tab's button
/// through the flush's `SPELL_UPDATE_COOLDOWN` alone.
#[test]
fn a_pet_gcd_sweeps_the_spellbooks_pet_tab_button() {
    benilla_formats::wow_data_or_skip!();
    use benilla_protocol::messages::{PetActionEntry, PET_ACT_ENABLED};

    let mut rig = rig(
        vec![(CLAW, claw_book_spell())],
        &[CLAW],
        Some(summoned_pet()),
    );
    {
        let world = rig.app.world_mut();
        // Our class, so the book reads a token; no `ChrClasses` row, so "PET".
        let me = world.resource::<crate::net::GuidIndex>().0[&ME];
        world
            .entity_mut(me)
            .insert(super::press_tests::player_with(&[(
                benilla_protocol::field::FIELD_UNIT_BYTES_0,
                3 << 8,
            )]));
        world.insert_resource(crate::chr_classes::ChrClassTable(Default::default()));
        world.resource_mut::<PetBar>().spells.spells = vec![PetActionEntry::from(
            CLAW | (u32::from(PET_ACT_ENABLED) << 24),
        )];
    }

    let mut script = benilla_ui::script::UiScript::new().unwrap();
    script.set_screen_size(1024.0, 768.0);
    crate::ui_script::test_ui::load_stock_ui(&mut script);
    listen(&script);
    // A session's clock past zero, as the rig's own VM: stock `Cooldown.lua` needs `start > 0`.
    script.set_now(10.0);
    rig.app.world_mut().insert_non_send_resource(script);

    // The book is read at the call, and the window opens on its pet tab.
    rig.frame();
    let live = |rig: &mut Rig, f: &dyn Fn(&mut benilla_ui::script::UiScript)| {
        crate::ui_script::live::with_live_world(rig.app.world_mut(), |s| f(s)).expect("the VM");
    };
    live(&mut rig, &|s| {
        s.run("ToggleSpellBook(BOOKTYPE_PET)").unwrap();
        s.resolve();
        assert!(
            s.eval::<bool>("return SpellBookFrame.bookType == BOOKTYPE_PET")
                .unwrap(),
            "the pet tab is open"
        );
        assert_eq!(
            s.eval::<String>("return SpellButton1SpellName:GetText()")
                .unwrap(),
            "Claw"
        );
        assert!(
            !s.eval::<bool>("return SpellButton1Cooldown:IsShown()")
                .unwrap(),
            "no cooldown yet"
        );
    });

    // A press on the bar's Claw starts the pet's GCD.
    rig.press(1);
    rig.frame();

    live(&mut rig, &|s| {
        s.resolve();
        assert!(
            s.eval::<bool>("return SpellButton1Cooldown:IsShown()")
                .unwrap(),
            "the GCD sweeps the spellbook's button"
        );
        // `SpellButton_UpdateButton` hands `GetSpellCooldown(id, bookType)` to `CooldownFrame_SetTimer`.
        assert!(
            (s.eval::<f64>(
                "local _, duration = GetSpellCooldown(SpellBook_GetSpellID(1), BOOKTYPE_PET) \
             return duration"
            )
            .unwrap()
                - 1.5)
                .abs()
                < 1e-9,
            "and it is the 1.5 s the press started"
        );
        let errors = s.errors();
        assert!(errors.is_empty(), "script errors: {errors:?}");
    });
}

/// A spell of no catalog record but the test's own, parked until its cooldown event.
const PARKED: u32 = 9_001;

/// Claw with a short GCD, so a press's record runs out inside a test.
fn brisk_claw() -> SpellDisplay {
    SpellDisplay {
        start_recovery_ms: 40,
        ..claw()
    }
}

/// A parked spell: `Attributes` bit 25, its timers wait for `SMSG_COOLDOWN_EVENT`.
fn parked() -> SpellDisplay {
    SpellDisplay {
        name: "Parked".into(),
        attributes: 1 << 25,
        ..Default::default()
    }
}

/// Long enough for every `brisk_claw` GCD to have run out.
fn let_the_gcds_run_out() {
    std::thread::sleep(Duration::from_millis(120));
}

/// Every pet GCD and every recovery the GO leg inserts is a record, and an insert marks the ones
/// that ran out: after forty presses whose GCDs have all run out, the next press leaves the list
/// its live records and its own, a running recovery and a parked record among them.
#[test]
fn an_insert_leaves_the_pets_list_its_live_records_alone() {
    let mut rig = rig(
        vec![(CLAW, brisk_claw()), (GROWL, growl()), (PARKED, parked())],
        &[CLAW, GROWL, PARKED],
        Some(summoned_pet()),
    );
    rig.receive(vec![
        go(PET, GROWL),
        // Ten milliseconds, parked until `SMSG_COOLDOWN_EVENT`.
        SessionEvent::SpellCooldowns {
            caster: PET,
            flags: 0,
            cooldowns: vec![(PARKED, 10)],
        },
    ]);
    assert_eq!(
        rig.record_count(),
        2,
        "Growl's recovery and the parked record"
    );
    for _ in 0..40 {
        rig.press(1);
    }
    let_the_gcds_run_out();

    rig.press(1);

    assert_eq!(
        rig.record_count(),
        43,
        "Growl's recovery, the parked record and every GCD, until a collection"
    );
    rig.frame();
    assert_eq!(rig.cooldown(2).1, 5.0, "Growl still sweeps its 5 s");
    let parked = rig.cooldown(3);
    assert!(
        parked.1 > 0.0 && parked.2 == 0,
        "the parked record reads disabled: {parked:?}"
    );
}

/// The reference flushes no event for a cooldown's end: each press's insert flushes once in its applier (`0x805ec3`, `0x805eda`), and nothing else. An insert that arms nothing changes nothing (`0x805e42`).
#[test]
fn an_insert_fires_only_its_own_flush() {
    let mut rig = rig(
        vec![(CLAW, brisk_claw()), (GROWL, growl())],
        &[CLAW, GROWL],
        Some(summoned_pet()),
    );
    for _ in 0..5 {
        rig.press(1);
    }
    assert_eq!(rig.frame(), flushes(5), "each press's own flush");
    let held = rig.generation();
    let_the_gcds_run_out();

    // Growl has no GCD pair: nothing is armed, so nothing is dropped or flushed.
    rig.press(2);
    assert!(
        rig.frame().is_empty(),
        "an insert that arms nothing fires nothing"
    );
    assert_eq!(rig.generation(), held);

    rig.press(1);
    assert_eq!(rig.frame(), FLUSH, "the insert's flush, once");
    assert_eq!(rig.generation(), held + 1, "the insert is the one mutation");
    assert!(rig.frame().is_empty(), "and nothing follows");
}

/// An insert after the GCDs ran out changes no `GetPetActionCooldown` read: a running recovery and
/// a parked record whose timers have run down push what they pushed before, the same start
/// included.
#[test]
fn an_insert_changes_no_bar_read_of_the_records_that_stay() {
    let mut rig = rig(
        vec![(CLAW, brisk_claw()), (GROWL, growl()), (PARKED, parked())],
        &[CLAW, GROWL, PARKED],
        Some(summoned_pet()),
    );
    rig.receive(vec![
        go(PET, GROWL),
        SessionEvent::SpellCooldowns {
            caster: PET,
            flags: 0,
            cooldowns: vec![(PARKED, 10)],
        },
    ]);
    for _ in 0..5 {
        rig.press(1);
    }
    // The two packets' flushes, then one per press (`0x807039`, `0x80fe4b`, `0x805eda`).
    assert_eq!(rig.frame(), flushes(7));
    let (running, parked_read) = (rig.cooldown(2), rig.cooldown(3));
    assert_eq!(running.1, 5.0, "Growl sweeps its 5 s");
    assert!(
        parked_read.1 > 0.0 && parked_read.2 == 0,
        "the parked record shows disabled: {parked_read:?}"
    );
    let_the_gcds_run_out();

    rig.press(1);
    rig.frame();

    assert_eq!(
        rig.cooldown(2),
        running,
        "the running recovery, its start too"
    );
    assert_eq!(rig.cooldown(3), parked_read, "the parked record");
}
