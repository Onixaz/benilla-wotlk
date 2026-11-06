//! Emote sounds: `SMSG_TEXT_EMOTE` plays the performer's race/sex voice kit (`EmotesTextSound`)
//! for the active player and the party only. Our own `/wave` goes out as `CMSG_TEXT_EMOTE` with the
//! voice file it picked and plays through the server echo, which every hearer reads the same file
//! from. The catalog also serves `crate::ui_chat` and `crate::creature_anim`.

use bevy::prelude::*;

use benilla_formats::EmoteSoundCatalog;
use benilla_protocol::messages::ObjectType;

use crate::entities::{AttachPoints, Creatures};
use crate::net::{ActiveMover, EmoteKind, EmoteMessage, ObjectStore, SelfGuid, SelfPlayer};
use crate::spell::group_relation::GroupRoster;
use crate::ui_social::SocialState;
use benilla_assets::{AssetSet, LockRecover, WorldAssets};
use benilla_world::schedule::WorldStage;

use super::kit::{
    play_kit_ext, stop_older_emote_voices, KitRef, Latch, PlayExtras, SoundCategory, SoundKits,
    Volume,
};
use super::{AudioListener, SoundConfig, SoundOutput};

/// The race and sex a unit's voice kit is picked by: the descriptor's while the display is native,
/// else the display's override (`0x6ddb3a`-`0x6ddb61`).
fn voice_race_sex(store: &ObjectStore, creatures: Option<&Creatures>) -> Option<(u8, u8)> {
    let native = store.0.unit_displayid() == store.0.unit_native_displayid();
    store
        .0
        .unit_displayid()
        .filter(|_| !native)
        .and_then(|d| u32::try_from(d).ok())
        .and_then(|d| creatures?.display_race_sex(d))
        .or_else(|| Some((store.0.unit_race()?, store.0.unit_gender()?)))
}

/// The emote-audio catalog (also the chat sender's `/name` → text-id resolver).
#[derive(Resource)]
pub(crate) struct EmoteSounds(pub(crate) EmoteSoundCatalog);

impl EmoteSounds {
    /// Resolve a `/command` name (case-insensitive) to its EmotesText id.
    pub(crate) fn text_id(&self, name: &str) -> Option<u32> {
        self.0.text_id(name)
    }

    /// The file index `CMSG_TEXT_EMOTE` carries (`0x6ddb6a`): one of the kit of our race and sex,
    /// `-1` for a kit with no files.
    pub(crate) fn pick_file_index(
        &self,
        text_id: u32,
        store: &ObjectStore,
        creatures: Option<&Creatures>,
        kits: Option<&mut SoundKits>,
    ) -> i32 {
        let kit = voice_race_sex(store, creatures)
            .and_then(|(race, sex)| self.0.voice(text_id, u32::from(race), u32::from(sex)))
            .filter(|&k| k != 0);
        match (kit, kits) {
            (Some(kit), Some(kits)) => kits.pick_file_index(kit),
            _ => -1,
        }
    }

    /// An `Emotes.dbc` id's `AnimID`.
    pub(crate) fn anim(&self, emote_id: u32) -> Option<u32> {
        self.0.anim(emote_id)
    }

    /// An `Emotes.dbc` row's raw `AnimID` for the `UNIT_NPC_EMOTESTATE` resolver, `0` included.
    pub(crate) fn state_anim(&self, emote_id: u32) -> Option<u32> {
        self.0.state_anim(emote_id)
    }

    /// A text emote's `Emotes.dbc` id (`EmotesText.dbc` `EmoteID`); 0 is chat-only (`/thank`).
    pub(crate) fn text_emote(&self, text_id: u32) -> Option<u32> {
        self.0.text_emote(text_id)
    }

    /// The `Emotes.dbc` id in one of the five hard-coded gesture slots.
    pub(crate) fn gesture(&self, slot: usize) -> Option<u32> {
        self.0.gesture(slot)
    }

    /// An `Emotes.dbc` id's raw `EmoteFlags`, read by the posture-eligibility gate.
    pub(crate) fn emote_flags(&self, emote_id: u32) -> Option<u32> {
        self.0.emote_flags(emote_id)
    }

    /// The stand state a posture emote (`EmoteSpecProc == 1`) sets, as in `DoEmote`'s state branch.
    pub(crate) fn posture_state(&self, emote_id: u32) -> Option<u32> {
        self.0.posture_state(emote_id)
    }

    /// The `$ESD` anim event's kit for a looping state emote: `EventSoundID` only when
    /// `EmoteSpecProc == 2` (`row[+0x10] == 2` in the handler), so a one-shot emote id in the state
    /// field stays silent.
    pub(crate) fn state_event_sound(&self, emote_id: u32) -> Option<u32> {
        (self.0.spec_proc(emote_id) == Some(2))
            .then(|| self.0.event_sound(emote_id))
            .flatten()
    }
}

fn load_emote_sounds(mut commands: Commands, assets: Option<Res<WorldAssets>>) {
    let Some(assets) = assets else { return };
    let loaded = {
        let mut chain = assets.chain.lock_recover();
        benilla_formats::load_emote_sound_catalog(&mut chain)
    };
    match loaded {
        Ok(cat) => {
            info!("sound: {} emote commands", cat.len());
            commands.insert_resource(EmoteSounds(cat));
        }
        Err(e) => warn!("sound: emote catalog failed to load: {e:#}"),
    }
}

/// The attachment the emote voice plays.
pub(super) const VOICE_ATTACH: u16 = 17;

/// Who hears a text emote's voice: the composer's tail plays it for relation class 0, the active
/// player, or 2, a player in one of the four party slots. A raid-only member is class 4, a hostile
/// player 6, a creature 1/3/5/7/8 and an unstreamed performer 9, none of which play. An ignored
/// performer never reaches the composer.
fn voice_audible(
    performer: u64,
    me: Option<u64>,
    performer_is_player: bool,
    roster: &GroupRoster,
    ignored: bool,
) -> bool {
    if ignored {
        return false;
    }
    Some(performer) == me || performer_is_player && roster.in_party(me, performer)
}

/// One play of `0x746d60`.
pub(super) struct EmoteVoice {
    pub(super) kit: u32,
    pub(super) unit: Entity,
    /// The unit's attachment `0x11`.
    pub(super) at: Vec3,
    /// The kit properties' file (`0x746f3b`): a non-negative index plays that file, `-1` the normal
    /// pick, which the `$CSD` event asks for (`0x732be0`).
    pub(super) file_index: i32,
    /// The two CVar tests, which the `$CSD` event asks for (`0x732be2`) and the text voice does not.
    pub(super) check_cvars: bool,
    /// The active player or the active mover.
    pub(super) own: bool,
}

/// A player at health below 1 plays nothing, on both paths (`0x746d80`).
fn dead_player(store: Option<&ObjectStore>) -> bool {
    store.is_some_and(|s| {
        s.0.object_type() == Some(ObjectType::Player) && s.0.unit_health() == Some(0)
    })
}

/// Where an emote voice sounds and its volume scale: your own with the listener on the character
/// plays flat at 0.65 (`0x746f66`, `0x746fa2`-`0x746fc6`), every other at the unit at full scale.
fn voice_placement(own: bool, listener_at_character: bool, at: Vec3) -> (Option<Vec3>, f32) {
    if own && listener_at_character {
        (None, 0.65)
    } else {
        (Some(at), 1.0)
    }
}

/// Play a voice, then stop the unit's previous emote voice (`[unit+0x934]`); a play that opens no
/// channel stops nothing. Shared by the text-emote voice and the `$CSD` anim event. A Death
/// Knight's takes its voice chain, except kit 6576 (`0x746f6f`).
pub(super) fn play_emote_voice(
    kits: &mut SoundKits,
    assets: &WorldAssets,
    out: &mut SoundOutput,
    config: &SoundConfig,
    listener: Vec3,
    voice: EmoteVoice,
    store: Option<&ObjectStore>,
) -> anyhow::Result<bool> {
    /// The one emote kit `0x746d60` leaves unfiltered.
    const UNFILTERED_EMOTE_KIT: u32 = 6576;
    let EmoteVoice {
        kit,
        unit,
        at,
        file_index,
        check_cvars,
        own,
    } = voice;
    if dead_player(store) {
        return Ok(false);
    }
    let pet = store.is_some_and(|s| s.0.unit_pet_number() != 0);
    if check_cvars && (!config.emote_sounds || (!config.pet_sounds && pet)) {
        return Ok(false);
    }
    let (pos, scale) = voice_placement(own, config.listener_at_character, at);
    let extras = PlayExtras {
        variant: usize::try_from(file_index).ok(),
        source: Some(unit),
        latch: Latch::EmoteVoice,
        volume_mult: Volume(scale),
        voice_filter: (kit != UNFILTERED_EMOTE_KIT)
            .then(|| super::kit::voice_filter(out, store))
            .flatten(),
        ..PlayExtras::default()
    };
    let opened = play_kit_ext(
        kits,
        assets,
        out,
        config,
        listener,
        KitRef::Id(kit),
        pos,
        SoundCategory::Sfx,
        extras,
    )?;
    if opened {
        stop_older_emote_voices(out, unit);
    }
    Ok(opened)
}

/// The text voice's form gate (`0x7470a3`): a shapeshift, a form row that is no stance (`0x721ca0`),
/// whose `SpellShapeshiftForm` flags carry `0x1000` plays no voice.
fn form_mutes_voice(form_flags: Option<u32>) -> bool {
    form_flags.is_some_and(|f| f & 1 == 0 && f & 0x1000 != 0)
}

/// Route a text emote to the performer's race/sex voice. A performer whose race/sex has not arrived
/// yet stays silent. `SMSG_EMOTE` plays no sound: the one-shot path never reads `EventSoundID`,
/// which only the state emote's `$ESD` does.
fn emote_sounds(
    mut msgs: MessageReader<EmoteMessage>,
    units: Query<(&ObjectStore, &Transform)>,
    owners: Query<(), Or<(With<SelfPlayer>, With<ActiveMover>)>>,
    spells: Option<Res<crate::ui_action::Spells>>,
    emotes: Option<Res<EmoteSounds>>,
    kits: Option<ResMut<SoundKits>>,
    assets: Option<Res<WorldAssets>>,
    creatures: Option<Res<Creatures>>,
    roster: Option<Res<GroupRoster>>,
    social: Res<SocialState>,
    self_guid: Res<SelfGuid>,
    attach: AttachPoints,
    mut out: NonSendMut<SoundOutput>,
    config: Res<SoundConfig>,
    listener: Res<AudioListener>,
) {
    if msgs.is_empty() {
        return;
    }
    let (Some(emotes), Some(mut kits), Some(assets)) = (emotes, kits, assets) else {
        return;
    };
    let listener = listener.pos;
    let no_roster = GroupRoster::default();
    for m in msgs.read() {
        let EmoteKind::Text {
            text_id,
            file_index,
        } = m.kind
        else {
            continue;
        };
        // The `Sound_EnableEmoteSounds` CVar gates the text-emote voice, read at play time; a zero never
        // fetches the kit.
        if !config.emote_sounds {
            continue;
        }
        let Some((unit, (store, transform))) =
            m.source.and_then(|e| units.get(e).ok().map(|row| (e, row)))
        else {
            continue;
        };
        let is_player = store.0.object_type() == Some(ObjectType::Player);
        if !voice_audible(
            m.guid,
            self_guid.0,
            is_player,
            roster.as_deref().unwrap_or(&no_roster),
            social.is_ignored(m.guid),
        ) {
            continue;
        }
        let form = u32::from(store.0.unit_shapeshift_form());
        let form_flags = spells
            .as_deref()
            .and_then(|s| s.forms.get(&form))
            .map(|f| f.flags);
        if form_mutes_voice(form_flags) {
            continue;
        }
        let Some((race, sex)) = voice_race_sex(store, creatures.as_deref()) else {
            continue;
        };
        let Some(kit) = emotes
            .0
            .voice(text_id, race as u32, sex as u32)
            .filter(|&k| k != 0)
        else {
            continue; // most emotes are voiceless (/wave)
        };
        let voice = EmoteVoice {
            kit,
            unit,
            at: attach.point(unit, VOICE_ATTACH, transform.translation),
            file_index,
            check_cvars: false,
            own: owners.contains(unit),
        };
        if let Err(e) = play_emote_voice(
            &mut kits,
            &assets,
            &mut out,
            &config,
            listener,
            voice,
            Some(store),
        ) {
            warn!("emote sound (kit {kit}): {e:#}");
        }
    }
}

/// Registration hook for [`super::SoundPlugin`].
pub(super) fn plugin(app: &mut App) {
    app.add_systems(Startup, load_emote_sounds.after(AssetSet::Open))
        .add_systems(Update, emote_sounds.in_set(WorldStage::Present));
}

#[cfg(test)]
mod tests {
    use super::super::kit::occupies_emote_slot;
    use super::*;

    const ME: u64 = 1;
    const MATE: u64 = 2;
    const RAIDER: u64 = 3;
    const STRANGER: u64 = 4;

    fn roster() -> GroupRoster {
        GroupRoster {
            party: vec![MATE],
            raid: vec![ME, MATE, RAIDER],
        }
    }

    /// Class 0 and class 2 play, every other class is silent.
    #[test]
    fn only_you_and_the_party_are_heard() {
        let r = roster();
        let heard = |who, player, ignored| voice_audible(who, Some(ME), player, &r, ignored);
        assert!(heard(ME, true, false), "class 0, you");
        assert!(heard(MATE, true, false), "class 2, a party member");
        assert!(
            !heard(RAIDER, true, false),
            "class 4: the raid is not the party slots"
        );
        assert!(
            !heard(STRANGER, true, false),
            "class 4 or 6, any other player"
        );
        assert!(
            !heard(MATE, false, false),
            "a creature never classes as a player"
        );
        assert!(!voice_audible(
            MATE,
            None,
            true,
            &GroupRoster::default(),
            false
        ));
    }

    /// An ignored performer is dropped before the composer, even you or a mate.
    #[test]
    fn an_ignored_performer_is_silent() {
        let r = roster();
        assert!(!voice_audible(MATE, Some(ME), true, &r, true));
        assert!(!voice_audible(ME, Some(ME), true, &r, true));
    }

    /// Your own voice plays flat at 0.65 with the listener on the character, positioned at full
    /// scale otherwise; any other unit's always positions.
    #[test]
    fn your_own_voice_plays_flat_at_the_reference_scale() {
        let at = Vec3::new(1.0, 2.0, 3.0);
        assert_eq!(voice_placement(true, true, at), (None, 0.65));
        assert_eq!(voice_placement(true, false, at), (Some(at), 1.0));
        assert_eq!(voice_placement(false, true, at), (Some(at), 1.0));
    }

    /// A player below 1 health plays nothing; a creature, or a player with no health field yet, is
    /// not this gate's.
    #[test]
    fn a_dead_player_plays_no_voice() {
        use benilla_protocol::field::FIELD_UNIT_HEALTH;
        use benilla_protocol::ObjectFields;
        let store = |type_mask, health| {
            ObjectStore(ObjectFields::from_pairs(&[
                (2, type_mask),
                (FIELD_UNIT_HEALTH, health),
            ]))
        };
        assert!(dead_player(Some(&store(0x19, 0))), "a dead player");
        assert!(!dead_player(Some(&store(0x19, 1))), "alive");
        assert!(!dead_player(Some(&store(0x9, 0))), "a dead creature");
        assert!(!dead_player(None));
    }

    /// Bit `0x1000` silences a shapeshift; a stance is no shapeshift, and no form row is no gate.
    #[test]
    fn a_flagged_form_mutes_the_text_voice() {
        assert!(form_mutes_voice(Some(0x1000)));
        assert!(!form_mutes_voice(Some(0x1001)), "a stance");
        assert!(!form_mutes_voice(Some(0)));
        assert!(!form_mutes_voice(None));
    }

    /// The sender's pick reads the kit of its race and sex; a kit-less emote sends `-1`.
    #[test]
    fn a_kit_with_no_voice_sends_minus_one() {
        let sounds = EmoteSounds(EmoteSoundCatalog::default());
        let store = ObjectStore::default();
        assert_eq!(sounds.pick_file_index(101, &store, None, None), -1);
    }

    /// The slot holder is the unit's own emote voice, never its bark or another unit's.
    #[test]
    fn the_emote_slot_is_the_units_own() {
        let mut world = World::new();
        let (a, b) = (world.spawn_empty().id(), world.spawn_empty().id());
        assert!(occupies_emote_slot(Some(a), Latch::EmoteVoice, a));
        assert!(!occupies_emote_slot(Some(b), Latch::EmoteVoice, a));
        assert!(!occupies_emote_slot(Some(a), Latch::Voice(0), a));
        assert!(!occupies_emote_slot(None, Latch::EmoteVoice, a));
    }
}
