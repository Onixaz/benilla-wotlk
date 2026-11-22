//! The app half of `benilla_ui::script::macros`: the icon chooser's catalog, the two
//! `macros-cache.txt` files (account-data slots 4 and 5, [`crate::account_data`]), the runner, and
//! the bound spell the action bar reads. The files load with the in-game UI and are written at its
//! shutdown when a macro was created, edited or deleted (`0x566a30`).

use bevy::prelude::*;

use benilla_ui::script::{MacroState, UiScript};

use crate::char_select::InWorldGated;
use benilla_assets::{LockRecover, WorldAssets};

pub(crate) mod run;
mod store;
#[cfg(test)]
mod tests;

/// The macro files' state: whether this VM has loaded them, and whether a save is owed.
#[derive(Resource, Default)]
pub(crate) struct MacroFiles {
    /// Loaded into this VM: a relog meets a fresh VM with an empty macro table.
    loaded: crate::ui_script::VmMemo<bool>,
    dirty: bool,
}

/// Macro index to its bound spell, the reference's `[rec+0x564]`, which an action-bar macro slot's
/// state reads through. A missing entry names no macro, which the bar greys, unlike a macro that
/// casts nothing.
#[derive(Resource, Default)]
pub(crate) struct MacroBoundSpells(pub(crate) std::collections::HashMap<u32, BoundSpell>);

pub(crate) use run::BoundSpell;

/// The icon chooser's list, built once for the process: 12340 keeps it in globals
/// (`[0xbeaf94]` count, `[0xbeaf98]` paths) that outlive every login and reload.
#[derive(Resource, Default)]
struct MacroIconList(std::sync::Arc<std::sync::OnceLock<Vec<String>>>);

/// The guild bank tab chooser's `INV_` list, built beside it by the macro icon build (`0x565840`).
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
                    // In-world only: the per-character file needs the character.
                    load_macros
                        .in_set(crate::ui_script::UiFeed)
                        .in_set(InWorldGated),
                    // After the script tick that dirtied the table.
                    note_dirty_macros.in_set(crate::ui_script::UiDrain),
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
    mut script: crate::ui_script::InTurn,
    assets: Option<Res<WorldAssets>>,
    list: Res<MacroIconList>,
    item_list: Res<MacroItemIconList>,
    mut seeded: Local<crate::ui_script::VmMemo<bool>>,
) {
    let (Some(mut script), Some(assets)) = (script.turn(), assets) else {
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
/// server (the reference's macro read `0x567c10` runs from the deferred load).
fn load_macros(
    mut script: crate::ui_script::InTurn,
    data: Res<crate::account_data::AccountData>,
    mut files: ResMut<MacroFiles>,
    table: Option<Res<crate::ui_chat::commands::SlashCommands>>,
    book: Res<crate::ui_spellbook::SpellBook>,
    mut bound: ResMut<MacroBoundSpells>,
) {
    use crate::account_data::store::{MACROS_ACCOUNT, MACROS_CHARACTER};
    let Some(mut script) = script.turn() else {
        return;
    };
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
    // The load binds every macro's spell before its bar signal (`0x567f8d`, `0x567fac`).
    if let Some(table) = table.as_deref() {
        bound.0 = bind_all(table, &state, &book.live.book);
        script.set_macro_bindings(vm_bindings(&bound.0));
    }
    script.set_macros(state);
    // `UPDATE_MACROS`: the macro frame redraws on it, as after an edit.
    script.fire_event("UPDATE_MACROS", vec![]);
}

/// An edit fires `UPDATE_MACROS` and owes the shutdown's save.
fn note_dirty_macros(mut script: crate::ui_script::InTurn, mut files: ResMut<MacroFiles>) {
    let Some(mut script) = script.turn() else {
        return;
    };
    if !script.take_macros_dirty() {
        return;
    }
    files.dirty = true;
    script.fire_event("UPDATE_MACROS", vec![]);
}

/// The macro save (`0x566a30`) at the UI shutdown: both files, if anything moved.
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

/// Every macro's bound spell off the book, by 1-based index (`0x565c40` -> `0x5650c0`).
pub(crate) fn bind_all(
    table: &crate::ui_chat::commands::SlashCommands,
    macros: &MacroState,
    book: &benilla_ui::script::SpellBookState,
) -> std::collections::HashMap<u32, BoundSpell> {
    (1..=benilla_ui::script::MAX_MACRO_INDEX as u32)
        .filter_map(|index| {
            let m = macros.get(index as usize)?;
            Some((index, run::bound_spell(table, &m.body, book)))
        })
        .collect()
}

/// A macro verb's binding change: a parsed macro looked up (`0x565c40` -> `0x5650c0`), a delete shifts its set's later bindings (`0x566d50`).
pub(crate) fn apply_change(
    table: &crate::ui_chat::commands::SlashCommands,
    macros: &MacroState,
    book: &benilla_ui::script::SpellBookState,
    bound: &mut std::collections::HashMap<u32, BoundSpell>,
    change: benilla_ui::script::MacroChange,
) {
    use benilla_ui::script::{MacroChange, MAX_ACCOUNT_MACROS, MAX_MACRO_INDEX};
    match change {
        MacroChange::Parsed(index) => match macros.get(index as usize) {
            Some(m) => {
                bound.insert(index, run::bound_spell(table, &m.body, book));
            }
            None => {
                bound.remove(&index);
            }
        },
        MacroChange::Deleted(index) => {
            let last = if index as usize <= MAX_ACCOUNT_MACROS {
                MAX_ACCOUNT_MACROS
            } else {
                MAX_MACRO_INDEX
            } as u32;
            bound.remove(&index);
            for later in index + 1..=last {
                if let Some(b) = bound.remove(&later) {
                    bound.insert(later - 1, b);
                }
            }
        }
    }
}

/// The VM's copy of `bound`, which `GetActionAutocast` reads; [`run::bound_spell`] resolves the player's book only.
pub(crate) fn vm_bindings(
    bound: &std::collections::HashMap<u32, BoundSpell>,
) -> std::collections::HashMap<u32, benilla_ui::script::MacroBinding> {
    bound
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
        .collect()
}

/// The rank replace's macro leg (`0x5aafd0`): a bar macro on a `replaces` spell re-points at `new`, no slot signal (`0x5ab0e2`-`0x5ab0f3`).
pub(crate) fn replace_rank(
    bound: &mut std::collections::HashMap<u32, BoundSpell>,
    bar: &crate::ui_action::ActionSlots,
    new: u32,
    replaces: impl Fn(u32) -> bool,
) -> bool {
    let mut moved = false;
    for b in bar
        .buttons
        .values()
        .filter(|b| b.kind == benilla_protocol::messages::ACTION_KIND_MACRO)
    {
        if let Some(spell) = bound.get_mut(&b.action) {
            if let BoundSpell::Spell(old) = *spell {
                if old != 0 && replaces(old) {
                    *spell = BoundSpell::Spell(new);
                    moved = true;
                }
            }
        }
    }
    moved
}

/// The VM's copy of [`MacroBoundSpells`] written from it, in turn with the handler's events.
pub(crate) fn sync_vm_bindings(world: &mut World) {
    let Some(bound) = world.get_resource::<MacroBoundSpells>() else {
        return;
    };
    let vm = vm_bindings(&bound.0);
    if let Some(mut script) = world.get_non_send_resource_mut::<UiScript>() {
        script.set_macro_bindings(vm);
    }
}

/// The book's macro pass (`0x53cea3` -> `0x566160`): each unresolved macro looked up and its slots signalled before the next, by index (#691) (`0x56617b`, `0x566193`).
pub(crate) fn book_rebind(world: &mut World) {
    rebind_from(world, 1);
}

/// The pass from 1-based `start`: the next macro to resolve stores its spell and queues its slot signals, then the rest.
fn rebind_from(world: &mut World, start: u32) {
    let Some(macros) = world
        .get_non_send_resource::<UiScript>()
        .map(UiScript::macros)
    else {
        return;
    };
    let (Some(table), Some(book), Some(bound)) = (
        world.get_resource::<crate::ui_chat::commands::SlashCommands>(),
        world.get_resource::<crate::ui_spellbook::SpellBook>(),
        world.get_resource::<MacroBoundSpells>(),
    ) else {
        return;
    };
    let mut unresolved: Vec<u32> = bound
        .0
        .iter()
        .filter(|&(&index, b)| index >= start && *b == BoundSpell::Unresolved)
        .map(|(&index, _)| index)
        .collect();
    unresolved.sort_unstable();
    let found = unresolved.into_iter().find_map(|index| {
        let m = macros.get(index as usize)?;
        match run::bound_spell(table, &m.body, &book.live.book) {
            now @ BoundSpell::Spell(_) => Some((index, now)),
            _ => None,
        }
    });
    let Some((index, now)) = found else {
        return;
    };
    debug!("ui_macro: the book update resolved macro {index}");
    world
        .resource_mut::<MacroBoundSpells>()
        .0
        .insert(index, now);
    sync_vm_bindings(world);
    let slots: Vec<u8> = world
        .get_resource::<crate::ui_action::ActionSlots>()
        .map(|bar| {
            let mut slots: Vec<u8> = bar
                .buttons
                .values()
                .filter(|b| {
                    b.kind == benilla_protocol::messages::ACTION_KIND_MACRO && b.action == index
                })
                .map(|b| b.slot)
                .collect();
            slots.sort_unstable();
            slots
        })
        .unwrap_or_default();
    for slot in slots {
        crate::ui_script::live::in_turn(world, move |world| {
            crate::ui_action::slots::slot_changed(world, slot)
        });
    }
    crate::ui_script::live::in_turn(world, move |world| rebind_from(world, index + 1));
}

/// The event each macro line is fired as: id `0x188`, whose registry slot is written once, with
/// this name.
/// Run a macro by its 1-based index; returns whether it held a line. The reference's runner fires
/// `EXECUTE_CHAT_LINE` per line and nothing else: the stock `ChatFrame1` sends each through its edit
/// box (`ChatFrame.lua:1343`), and an addon registered for the event sees every macro line.
pub(crate) fn run_macro(script: &mut UiScript, index: u32) -> bool {
    let Some(body) = script.macros().get(index as usize).map(|m| m.body.clone()) else {
        return false;
    };
    if run::macro_lines(&body).next().is_none() {
        return false;
    }
    debug!("ui_macro: running macro {index}");
    script.run_macro_body(&body);
    true
}
