//! `glue-realmlist`: the stock realm list over the login screen with realms in two categories,
//! which the live server cannot show (it lists one). The list is asked for as Change Realm asks
//! (`RequestRealmList(1)`), then answered as the realm park answers; the tabs are named from the
//! install's own Region.

use bevy::prelude::*;

use benilla_protocol::RealmInfo;

use super::CaptureCtx;

/// The scenario's name in [`super::scenarios::GLUE_SCENARIOS`].
pub(super) const NAME: &str = "glue-realmlist";

/// Invented realms: three on category byte 1, two on byte 2, a mix of types, loads and counts.
fn fixture() -> Vec<RealmInfo> {
    let realm =
        |name: &str, category: u8, realm_type: u32, population: f32, characters: u8| RealmInfo {
            name: name.into(),
            address: "127.0.0.1:8085".into(),
            population,
            characters,
            realm_type,
            flags: 0,
            category,
            id: 0,
        };
    vec![
        realm("Amberfield", 1, 0, 0.5, 2),
        realm("Brightwater", 1, 1, 1.5, 0),
        realm("Copperdale", 1, 6, 1.0, 0),
        realm("Dunmoor", 2, 1, 0.8, 0),
        realm("Eisenwald", 2, 0, 1.2, 0),
    ]
}

/// Ask for the list on the stock screen once the glue VM stands, then answer it.
pub(super) fn seed_realm_list(
    ctx: Res<CaptureCtx>,
    mut step: Local<u8>,
    mut lists: MessageWriter<crate::net::RealmListMessage>,
    script: Option<NonSend<benilla_ui::script::UiScript>>,
) {
    if ctx.name != NAME {
        return;
    }
    match *step {
        0 => {
            let Some(s) = super::glue_press::glue(script.as_deref()) else {
                return;
            };
            if let Err(e) = s.run("RequestRealmList(1)") {
                warn!("glue-realmlist: RequestRealmList raised: {e}");
            }
        }
        2 => {
            lists.write(crate::net::RealmListMessage {
                realms: fixture(),
                failed: false,
            });
        }
        3.. => return,
        _ => {}
    }
    *step += 1;
}
