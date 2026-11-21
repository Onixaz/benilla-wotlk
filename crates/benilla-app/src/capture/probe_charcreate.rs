//! The character-create live probe (`WOW_PROBE_CHARCREATE="<name>[,race,class,gender[,skin,\
//! face,hair,haircolor,facial]]"`), inert without the env. At character select it opens the stock
//! create screen, puts the body up and presses Okay, logs the `SMSG_CHAR_CREATE` result byte,
//! then deletes the new character on the select screen and logs that result too, unless
//! `WOW_PROBE_CHARCREATE_KEEP=1`. Run it without
//! `WOW_CHAR`, which would enter the world; the switches are `docs/CONTRIBUTING.md`, "Running it
//! unattended".

use benilla_protocol::{messages, CharAction, CharCreateReq};
use bevy::prelude::*;

use crate::net::CharActionResultMessage;

pub(crate) struct ProbeCharCreatePlugin;

impl Plugin for ProbeCharCreatePlugin {
    fn build(&self, app: &mut App) {
        let Some(req) = std::env::var("WOW_PROBE_CHARCREATE")
            .ok()
            .as_deref()
            .and_then(parse_req)
        else {
            return; // inert without a valid env spec
        };
        let keep = std::env::var("WOW_PROBE_CHARCREATE_KEEP").as_deref() == Ok("1");
        app.insert_resource(CharCreateProbe {
            req,
            keep,
            state: ProbeState::AwaitingSelect,
            cleanup_since: None,
        })
        .add_systems(Update, drive_charcreate_probe);
    }
}

/// Parse `name[,race,class,gender[,skin,face,hair,haircolor,facial]]` into a request; the name is
/// required, everything else defaults (Human Warrior male, all appearance dials 0).
fn parse_req(spec: &str) -> Option<CharCreateReq> {
    let mut parts = spec.split(',').map(str::trim);
    let name = parts.next().filter(|n| !n.is_empty())?.to_string();
    let mut next_u8 = |default: u8| parts.next().and_then(|p| p.parse().ok()).unwrap_or(default);
    Some(CharCreateReq {
        name,
        race: next_u8(messages::RACE_HUMAN),
        class: next_u8(messages::CLASS_WARRIOR),
        gender: next_u8(messages::GENDER_MALE),
        skin: next_u8(0),
        face: next_u8(0),
        hair_style: next_u8(0),
        hair_color: next_u8(0),
        facial_hair: next_u8(0),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProbeState {
    /// Waiting for the stock select screen to stand idle.
    AwaitingSelect,
    /// The create screen asked for, waiting for it to stand.
    CreateScreen,
    /// Okay pressed, waiting for the create's result.
    Creating,
    /// Created: back at select, to delete it.
    Cleanup,
    /// The delete pressed, waiting for its result.
    Deleting,
    /// Nothing left to do.
    Done,
}

#[derive(Resource)]
struct CharCreateProbe {
    req: CharCreateReq,
    keep: bool,
    state: ProbeState,
    /// When the cleanup began waiting for the fresh roster to list the new character.
    cleanup_since: Option<f32>,
}

/// Drive the create, then the delete, through the stock screens as a player's clicks would.
fn drive_charcreate_probe(
    world: &mut World,
    results: &mut bevy::ecs::system::SystemState<MessageReader<CharActionResultMessage>>,
) {
    let results: Vec<(CharAction, u8)> = results
        .get_mut(world)
        .read()
        .map(|r| (r.action, r.code))
        .collect();
    let Some(mut probe) = world.remove_resource::<CharCreateProbe>() else {
        return;
    };
    let files = (
        world
            .get_resource::<crate::entities::CharCreate>()
            .and_then(|c| c.0.race_file(probe.req.race).map(str::to_owned)),
        world
            .get_resource::<crate::chr_classes::ChrClassTable>()
            .and_then(|c| c.0.file(u32::from(probe.req.class)).map(str::to_owned)),
    );
    let now = super::probes::probe_secs(world);
    super::probes::with_vm(world, |script| {
        probe_step(&mut probe, script.as_deref(), &files, &results, now);
    });
    world.insert_resource(probe);
}

/// One frame of the probe on the glue VM, with the frame's create and delete results.
fn probe_step(
    probe: &mut CharCreateProbe,
    script: Option<&benilla_ui::script::UiScript>,
    files: &(Option<String>, Option<String>),
    results: &[(CharAction, u8)],
    now: f32,
) {
    let glue = super::glue_press::glue(script);
    match probe.state {
        ProbeState::AwaitingSelect => {
            let Some(s) = glue.filter(|s| super::glue_press::select_up(s)) else {
                return;
            };
            info!(
                "probe-charcreate: at select — creating {:?} (race {} class {} gender {}; appearance {}/{}/{}/{}/{})",
                probe.req.name,
                probe.req.race,
                probe.req.class,
                probe.req.gender,
                probe.req.skin,
                probe.req.face,
                probe.req.hair_style,
                probe.req.hair_color,
                probe.req.facial_hair,
            );
            super::glue_press::press_create_screen(s);
            probe.state = ProbeState::CreateScreen;
        }
        ProbeState::CreateScreen => {
            let Some(s) = glue.filter(|s| super::glue_press::create_up(s)) else {
                return;
            };
            let (Some(race_file), Some(class_file)) = files else {
                error!("probe-charcreate: no file string for the race or class");
                probe.state = ProbeState::Done;
                return;
            };
            match super::glue_press::press_body(s, &probe.req, race_file, class_file) {
                Ok(()) => {
                    info!(
                        "probe-charcreate: dials pressed to {:?}",
                        s.glue_create_look().map(|l| l.character.dials)
                    );
                    super::glue_press::press_okay(s, &probe.req.name);
                    probe.state = ProbeState::Creating;
                }
                Err(why) => {
                    error!("probe-charcreate: {why}");
                    probe.state = ProbeState::Done;
                }
            }
        }
        ProbeState::Creating => {
            let Some(&(_, code)) = results.iter().find(|r| r.0 == CharAction::Create) else {
                return;
            };
            info!(
                "probe-charcreate: create result = {:#04x} ({})",
                code,
                result_label(CharAction::Create, code)
            );
            probe.state = match (code == messages::CHAR_CREATE_SUCCESS, probe.keep) {
                (true, false) => ProbeState::Cleanup,
                (true, true) => {
                    info!("probe-charcreate: KEEP set — leaving the new character in place");
                    ProbeState::Done
                }
                (false, _) => ProbeState::Done,
            };
        }
        ProbeState::Cleanup => {
            let Some(s) = glue.filter(|s| super::glue_press::select_up(s)) else {
                return;
            };
            let roster = super::glue_press::roster(s);
            match roster
                .iter()
                .position(|(n, _)| n.eq_ignore_ascii_case(&probe.req.name))
            {
                Some(row) => {
                    info!("probe-charcreate: cleaning up — deleting row {}", row + 1);
                    super::glue_press::press_delete(s, row);
                    probe.state = ProbeState::Deleting;
                }
                // The screen stands before the roster its create asked for lands.
                None if now - *probe.cleanup_since.get_or_insert(now) < CLEANUP_WAIT_SECS => {}
                None => {
                    warn!("probe-charcreate: created char not in roster — cannot clean up");
                    probe.state = ProbeState::Done;
                }
            }
        }
        ProbeState::Deleting => {
            let Some(&(_, code)) = results.iter().find(|r| r.0 == CharAction::Delete) else {
                return;
            };
            info!(
                "probe-charcreate: delete result = {:#04x} ({})",
                code,
                result_label(CharAction::Delete, code)
            );
            probe.state = ProbeState::Done;
        }
        ProbeState::Done => {}
    }
}

/// How long the cleanup waits for the fresh roster to list the new character.
const CLEANUP_WAIT_SECS: f32 = 15.0;

/// A name for the result bytes the probe checks; the create screen has the full table.
fn result_label(action: CharAction, code: u8) -> &'static str {
    match (action, code) {
        (CharAction::Create, messages::CHAR_CREATE_SUCCESS) => "CHAR_CREATE_SUCCESS",
        (CharAction::Create, messages::CHAR_CREATE_NAME_IN_USE) => "CHAR_NAME_ALREADY_IN_USE",
        (CharAction::Delete, messages::CHAR_DELETE_SUCCESS) => "CHAR_DELETE_SUCCESS",
        _ => "other",
    }
}
