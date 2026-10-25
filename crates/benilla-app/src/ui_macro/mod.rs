//! The app half of `benilla_ui::script::macros`: the icon chooser's catalog, the two
//! `macros-cache.txt` files (account-data slots 4 and 5, [`crate::account_data`]), the runner, and
//! the bound spell the action bar reads. The files load with the in-game UI and are written at its
//! shutdown when a macro was created, edited or deleted (`CGUIMacros::SaveMacros` `0x566a30`).

use bevy::prelude::*;

use benilla_ui::script::{MacroState, ScriptValue, UiScript};

use crate::char_select::InWorldGated;
use benilla_assets::{LockRecover, WorldAssets};

pub(crate) mod run;
mod store;
#[cfg(test)]
mod tests;

/// The macro files' state: whether this VM has loaded them, and whether a save is owed
/// (`m_saveMacros`).
#[derive(Resource, Default)]
pub(crate) struct MacroFiles {
    /// Loaded into this VM: a relog meets a fresh VM with an empty macro table.
    loaded: crate::ui_script::VmMemo<bool>,
    dirty: bool,
}

/// Macro index to its bound spell, the reference's `[rec+0x564]` (read by `0x4e5ba0`), which an
/// action-bar macro slot's state reads through. A missing entry names no macro, which `0x4e5030`
/// greys, unlike a macro that casts nothing.
#[derive(Resource, Default)]
pub(crate) struct MacroBoundSpells(pub(crate) std::collections::HashMap<u32, BoundSpell>);

pub(crate) use run::BoundSpell;

/// The icon chooser's list, built once for the process: 12340 keeps it in globals
/// (`[0xbeaf94]` count, `[0xbeaf98]` paths) that outlive every login and reload.
#[derive(Resource, Default)]
struct MacroIconList(std::sync::Arc<std::sync::OnceLock<Vec<String>>>);

/// The guild bank tab chooser's `INV_` list, built beside it (`MacroIcons_Build 0x565840`).
#[derive(Resource, Default)]
struct MacroItemIconList(std::sync::Arc<std::sync::OnceLock<Vec<String>>>);

pub(crate) struct UiMacroPlugin;

impl Plugin for UiMacroPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MacroFiles>()
            .init_resource::<MacroBoundSpells>()
            .init_resource::<MacroIconList>()
            .init_resource::<MacroItemIconList>()
            .add_systems(
                Update,
                (
                    // Once per VM, in `Update`: it needs both the patch chain and the VM, and each
                    // login builds a fresh VM.
                    load_icon_catalog.in_set(crate::ui_script::UiFeed),
                    // Before the action feeds read it (they run in `UnitFeed`), so a macro edited
                    // this frame reports its new spell's cooldown the same frame.
                    rebind_macro_spells
                        .in_set(crate::ui_script::UiFeed)
                        .before(crate::ui_unit::UnitFeed),
                    // In-world only: the per-character file needs the character.
                    load_macros
                        .in_set(crate::ui_script::UiFeed)
                        .in_set(InWorldGated),
                    // After the script tick that dirtied the table.
                    note_dirty_macros.after(crate::ui_script::UiInput),
                ),
            );
    }
}

/// Give each VM the icon chooser's builder: the `Spell_` and `Ability_` files under
/// `Interface\Icons\` ([`benilla_formats::load_macro_icons`]), listed on the first
/// `GetNumMacroIcons` into [`MacroIconList`]. The listing walks every archive's listfile, ~0.6 s,
/// so no VM builds it unasked. A failed load leaves it empty, and `MacroPopupOkayButton_Update`
/// then keeps OKAY disabled.
fn load_icon_catalog(
    script: Option<NonSendMut<UiScript>>,
    assets: Option<Res<WorldAssets>>,
    list: Res<MacroIconList>,
    item_list: Res<MacroItemIconList>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (Some(mut script), Some(assets)) = (script, assets) else {
        return;
    };
    if !seeded.claim(&script) {
        return;
    }
    let chain = assets.chain.clone();
    let built = std::sync::Arc::clone(&list.0);
    let item_chain = chain.clone();
    script.set_macro_icon_source(Box::new(move || {
        built
            .get_or_init(|| list_icons(&mut chain.lock_recover()))
            .clone()
    }));
    let item_built = std::sync::Arc::clone(&item_list.0);
    script.set_macro_item_icon_source(Box::new(move || {
        item_built
            .get_or_init(|| list_item_icons(&mut item_chain.lock_recover()))
            .clone()
    }));
}

fn list_item_icons(chain: &mut benilla_formats::Chain) -> Vec<String> {
    match benilla_formats::load_macro_item_icons(chain) {
        Ok(icons) => {
            info!("ui_macro: {} item icons in the tab chooser", icons.len());
            icons
        }
        Err(e) => {
            error!("ui_macro: no Interface\\Icons\\ listing — the tab chooser is empty: {e:#}");
            Vec::new()
        }
    }
}

fn list_icons(chain: &mut benilla_formats::Chain) -> Vec<String> {
    match benilla_formats::load_macro_icons(chain) {
        Ok(icons) => {
            info!("ui_macro: {} macro icons in the chooser", icons.len());
            icons
        }
        Err(e) => {
            error!("ui_macro: no Interface\\Icons\\ listing — the chooser is empty: {e:#}");
            Vec::new()
        }
    }
}

/// `(realm, character)` off the roster's login pick, keying the per-character files.
pub(crate) fn identity(roster: &crate::char_select::Roster) -> Option<(String, String)> {
    let guid = roster.pending_pick?;
    let name = roster.chars.iter().find(|c| c.guid == guid)?.name.clone();
    let realm = roster
        .realm
        .as_ref()
        .map(|r| r.name.clone())
        .unwrap_or_else(|| "Realm".into());
    Some((realm, name))
}

/// Seed the engine's macro table from the two slots once per VM, when both have heard from the
/// server (`LoadMacrosCallback` `0x567c10` runs from the deferred load).
fn load_macros(
    script: Option<NonSendMut<UiScript>>,
    data: Res<crate::account_data::AccountData>,
    mut files: ResMut<MacroFiles>,
) {
    use crate::account_data::store::{MACROS_ACCOUNT, MACROS_CHARACTER};
    let Some(mut script) = script else { return };
    if *files.loaded.get(&script) {
        return;
    }
    let (Some(account), Some(character)) = (data.read(MACROS_ACCOUNT), data.read(MACROS_CHARACTER))
    else {
        return; // waiting for the server's times
    };
    *files.loaded.get(&script) = true;
    files.dirty = false;
    let state = MacroState {
        account: store::parse(&String::from_utf8_lossy(&account), 0),
        character: store::parse(&String::from_utf8_lossy(&character), store::CHARACTER_BASE),
    };
    info!(
        "ui_macro: {} account + {} character macros",
        state.account.len(),
        state.character.len()
    );
    script.set_macros(state);
    // `UPDATE_MACROS` (string at `0x852460`): the macro frame redraws on it, as after an edit.
    script.fire_event("UPDATE_MACROS", vec![]);
}

/// An edit fires `UPDATE_MACROS` and owes the shutdown's save.
fn note_dirty_macros(script: Option<NonSendMut<UiScript>>, mut files: ResMut<MacroFiles>) {
    let Some(mut script) = script else { return };
    if !script.take_macros_dirty() {
        return;
    }
    files.dirty = true;
    script.fire_event("UPDATE_MACROS", vec![]);
}

/// `CGUIMacros::SaveMacros` (`0x566a30`) at the UI shutdown: both files, if anything moved.
pub(crate) fn save_to_account_data(world: &mut World) {
    use crate::account_data::store::{MACROS_ACCOUNT, MACROS_CHARACTER};
    let dirty = world.get_resource::<MacroFiles>().is_some_and(|f| f.dirty)
        || world
            .get_non_send_resource_mut::<UiScript>()
            .is_some_and(|mut s| s.take_macros_dirty());
    if !dirty {
        return;
    }
    let Some(state) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::macros)
    else {
        return;
    };
    let account = store::write(&state.account, 0);
    let character = store::write(&state.character, store::CHARACTER_BASE);
    crate::account_data::save_slots(
        world,
        &[
            (MACROS_ACCOUNT, account.into_bytes()),
            (MACROS_CHARACTER, character.into_bytes()),
        ],
    );
    if let Some(mut files) = world.get_resource_mut::<MacroFiles>() {
        files.dirty = false;
    }
}

/// Recompute every macro's bound spell when the macro table or the spell book changes: gated on
/// the engine's macro generation and on change detection over the action store.
fn rebind_macro_spells(
    script: Option<NonSendMut<UiScript>>,
    actions: Res<crate::ui_action::PlayerActions>,
    table: Option<Res<crate::ui_chat::commands::SlashCommands>>,
    mut bound: ResMut<MacroBoundSpells>,
    mut last_generation: Local<crate::ui_script::VmMemo<Option<u64>>>,
) {
    let (Some(mut script), Some(table)) = (script, table) else {
        return;
    };
    // Per VM: a fresh VM restarts its generation at 0, which a bare memo would gate off for good.
    let last_generation = last_generation.get(&script);
    let generation = script.macros_generation();
    if *last_generation == Some(generation) && !actions.is_changed() {
        return;
    }
    *last_generation = Some(generation);

    let (macros, book) = (script.macros(), script.spellbook());
    let mut fresh = std::collections::HashMap::new();
    for index in 1..=(benilla_ui::script::MAX_MACROS as u32 * 2) {
        let Some(m) = macros.get(index as usize) else {
            continue;
        };
        fresh.insert(index, run::bound_spell(&table, &m.body, &book));
    }
    // The engine's copy, for `GetActionAutocast`, re-pushed on every recompute so a fresh VM has
    // it. The book flag stays the player's: `run::bound_spell` resolves the player's book only.
    script.set_macro_bindings(
        fresh
            .iter()
            .map(|(&index, b)| {
                let spell = match *b {
                    BoundSpell::None => 0,
                    BoundSpell::Unresolved => -1,
                    BoundSpell::Spell(id) => id as i32,
                };
                (
                    index,
                    benilla_ui::script::MacroBinding {
                        spell,
                        pet_book: false,
                    },
                )
            })
            .collect(),
    );
    if fresh != bound.0 {
        let spells = fresh
            .values()
            .filter(|b| matches!(b, BoundSpell::Spell(_)))
            .count();
        debug!(
            "ui_macro: {} macro(s), {spells} bound to a spell",
            fresh.len()
        );
        bound.0 = fresh;
    }
}

/// The event each macro line is fired as: id `0x188`, whose registry slot `0xbe17b8` is written
/// once, at `0x51b4ff`, with the string at `0x852470`.
const EXECUTE_CHAT_LINE: &str = "EXECUTE_CHAT_LINE";

/// Run a macro by its 1-based index; returns whether anything ran. Like the reference's runner
/// (`0x4f14e0`, reached only from `UseAction` at `0x4e6098`), it fires `EXECUTE_CHAT_LINE` per
/// non-empty line and nothing else: the stock `ChatFrame1` sends each through its edit box
/// (`ChatFrame.lua:1343`), and an addon registered for the event sees every macro line.
pub(crate) fn run_macro(script: &mut UiScript, index: u32) -> bool {
    let Some(body) = script.macros().get(index as usize).map(|m| m.body.clone()) else {
        return false;
    };
    let lines: Vec<String> = run::macro_lines(&body).map(str::to_string).collect();
    if lines.is_empty() {
        return false;
    }
    debug!("ui_macro: running macro {index} ({} line(s))", lines.len());
    for line in lines {
        script.fire_event(EXECUTE_CHAT_LINE, vec![ScriptValue::Str(line)]);
    }
    true
}
