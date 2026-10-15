//! The totem bar's element lists (`0xbe8ea0`) and their refill into slots 132..143 (`0x5ab9d0`).

use bevy::prelude::*;

use benilla_formats::{SpellCatalog, SpellDisplay};
use benilla_protocol::messages::ACTION_KIND_SPELL;

use crate::cvars::Cvars;
use crate::net::NetCommands;
use crate::ui_action::PlayerActions;

/// The calls and Totemic Recall (`0xaccccc`): their learn or unlearn refills the bar.
pub(crate) const CALL_SPELLS: [u32; 4] = [66842, 66843, 66844, 36936];
/// `AttributesEx7 & 0x20`: the spell is a totem, filed by its `TotemCategory` (`0x5428ad`).
const ATTR_EX7_TOTEM: u32 = 0x20;
/// Action slot 132 (`0x84`), the first of three pages of four element slots.
pub(crate) const FIRST_TOTEM_SLOT: u8 = 132;
/// The CVar whose bits record the elements already seeded (`0x51f3ae`).
pub(crate) const CVAR_AUTO_FILLED: &str = "autoFilledMultiCastSlots";

/// The element bits a tool row stands for (`0x5a7b50`): bit 0 fire, 1 earth, 2 water, 3 air.
fn element_mask(category: u32) -> u32 {
    match category {
        2 => 0x2,
        3 => 0x8,
        4 => 0x1,
        5 => 0x4,
        21 => 0xf,
        _ => 0,
    }
}

/// The four lists, and whether a refill is due.
#[derive(Resource, Default, Debug)]
pub(crate) struct MultiCastLists {
    pub(crate) lists: [Vec<u32>; 4],
    /// Set by a live learn or any unlearn that calls for `0x5ab9d0`.
    pub(crate) refill_due: bool,
    /// Set by a refill: the UI fires `UPDATE_MULTI_CAST_ACTIONBAR` (`0x5abaaa`).
    pub(crate) announce: bool,
}

impl MultiCastLists {
    /// The registrar's totem arm (`0x5428ad`): files by element, ranks up in place; true if changed.
    pub(crate) fn learn(&mut self, d: &SpellDisplay, catalog: &SpellCatalog) -> bool {
        if d.attributes_ex7 & ATTR_EX7_TOTEM == 0 {
            return false;
        }
        let mut changed = false;
        for category in d.totem_categories {
            let mask = element_mask(category);
            for (i, list) in self.lists.iter_mut().enumerate() {
                if mask & (1 << i) == 0 || list.contains(&d.id) {
                    continue;
                }
                if d.rank_number() == 0 && d.spell_level == 0 {
                    list.push(d.id);
                    changed = true;
                    continue;
                }
                let key = d.rank_or_level();
                let same = list
                    .iter()
                    .position(|&o| catalog.get(o).is_some_and(|o| o.name == d.name));
                match same {
                    None => {
                        list.push(d.id);
                        changed = true;
                    }
                    Some(j) => {
                        if catalog
                            .get(list[j])
                            .is_some_and(|o| o.rank_or_level() < key)
                        {
                            list[j] = d.id;
                            changed = true;
                        }
                    }
                }
            }
        }
        changed
    }

    /// The unlearn's sweep (`0x53fe57`): out of every list; true when one is left empty.
    pub(crate) fn unlearn(&mut self, spell_id: u32) -> bool {
        let mut emptied = false;
        for list in &mut self.lists {
            if let Some(j) = list.iter().position(|&s| s == spell_id) {
                list.remove(j);
                emptied |= list.is_empty();
            }
        }
        emptied
    }
}

/// `0x5ab9d0`: an unseeded element's first spell fills its empty slots; returns the new mask.
pub(crate) fn refill(
    lists: &MultiCastLists,
    actions: &mut PlayerActions,
    net: &NetCommands,
    mask: u32,
) -> u32 {
    let mut next = mask;
    for (i, list) in lists.lists.iter().enumerate() {
        let bit = 1 << i;
        if mask & bit == 0 {
            if let Some(&spell) = list.first() {
                let packed = spell | (u32::from(ACTION_KIND_SPELL) << 24);
                for slot in (FIRST_TOTEM_SLOT + i as u8..FIRST_TOTEM_SLOT + 12).step_by(4) {
                    if !actions.buttons.contains_key(&slot) {
                        actions.write_slot(slot, packed, net);
                    }
                }
                next |= bit;
            }
        } else if list.is_empty() {
            next &= !bit;
        }
    }
    next
}

/// Runs a due refill, saving the mask when it moved.
pub(crate) fn run_refill(
    mut lists: ResMut<MultiCastLists>,
    mut actions: ResMut<PlayerActions>,
    net: Res<NetCommands>,
    mut cvars: ResMut<Cvars>,
) {
    if !lists.refill_due {
        return;
    }
    lists.refill_due = false;
    let mask = cvars.num(CVAR_AUTO_FILLED).map_or(0, |v| v as u32);
    let next = refill(&lists, &mut actions, &net, mask);
    if next != mask {
        cvars.set(CVAR_AUTO_FILLED, &next.to_string());
    }
    debug!("multicast: refilled, mask {mask:#x} -> {next:#x}");
    lists.announce = true;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn totem(id: u32, name: &str, rank: Option<&str>, category: u32) -> (u32, SpellDisplay) {
        (
            id,
            SpellDisplay {
                id,
                name: name.into(),
                rank: rank.map(str::to_string),
                attributes_ex7: ATTR_EX7_TOTEM,
                totem_categories: [category, 0],
                ..Default::default()
            },
        )
    }

    fn catalog() -> SpellCatalog {
        SpellCatalog::from_displays(
            [
                totem(3599, "Searing Totem", Some("Rank 1"), 4),
                totem(6363, "Searing Totem", Some("Rank 2"), 4),
                totem(8071, "Stoneskin Totem", Some("Rank 1"), 2),
                totem(2484, "Earthbind Totem", None, 2),
                totem(5394, "Healing Stream Totem", Some("Rank 1"), 5),
                totem(9999, "Every Element", None, 21),
                (
                    133,
                    SpellDisplay {
                        id: 133,
                        name: "Fireball".into(),
                        ..Default::default()
                    },
                ),
            ]
            .into_iter()
            .collect(),
        )
    }

    fn learn_all(lists: &mut MultiCastLists, cat: &SpellCatalog, ids: &[u32]) {
        for id in ids {
            lists.learn(cat.get(*id).unwrap(), cat);
        }
    }

    #[test]
    fn totems_file_by_element_and_a_rank_up_replaces_in_place() {
        let cat = catalog();
        let mut lists = MultiCastLists::default();
        learn_all(&mut lists, &cat, &[8071, 3599, 2484, 5394, 133]);
        assert_eq!(lists.lists[0], vec![3599], "fire");
        assert_eq!(lists.lists[1], vec![8071, 2484], "earth, in learn order");
        assert_eq!(lists.lists[2], vec![5394], "water");
        assert!(lists.lists[3].is_empty(), "air");
        assert!(
            lists.learn(cat.get(6363).unwrap(), &cat),
            "rank 2 changes the list"
        );
        assert_eq!(lists.lists[0], vec![6363]);
        assert!(
            !lists.learn(cat.get(3599).unwrap(), &cat),
            "rank 1 again keeps rank 2"
        );
        assert_eq!(lists.lists[0], vec![6363]);
    }

    #[test]
    fn a_master_tool_files_in_every_list_and_unlearn_reports_an_emptied_list() {
        let cat = catalog();
        let mut lists = MultiCastLists::default();
        learn_all(&mut lists, &cat, &[9999, 8071]);
        for list in &lists.lists {
            assert_eq!(list[0], 9999);
        }
        assert!(lists.unlearn(9999), "fire, water and air empty");
        assert_eq!(lists.lists[1], vec![8071]);
        assert!(lists.unlearn(8071), "earth empties too");
        assert!(lists.lists.iter().all(Vec::is_empty));
    }

    #[test]
    fn the_refill_seeds_each_new_element_once_and_skips_taken_slots() {
        let cat = catalog();
        let mut lists = MultiCastLists::default();
        learn_all(&mut lists, &cat, &[8071]);
        let (tx, rx) = crossbeam_channel::unbounded();
        let net = NetCommands(tx);
        let mut actions = PlayerActions::default();
        // Page 2's earth slot is taken already.
        actions.write_slot(137, 2484 | (u32::from(ACTION_KIND_SPELL) << 24), &net);
        rx.try_iter().for_each(drop);

        let mask = refill(&lists, &mut actions, &net, 0);
        assert_eq!(mask, 0x2, "earth seeded");
        let mut sent: Vec<u8> = rx
            .try_iter()
            .map(|c| match c {
                crate::net::ClientCommand::SetActionButton { button, .. } => button,
                other => panic!("{other:?}"),
            })
            .collect();
        sent.sort_unstable();
        assert_eq!(sent, vec![133, 141], "pages 1 and 3; page 2 was taken");

        // A seeded element is not refilled after the player clears its slot.
        actions.write_slot(133, 0, &net);
        rx.try_iter().for_each(drop);
        assert_eq!(refill(&lists, &mut actions, &net, mask), 0x2);
        assert!(!actions.buttons.contains_key(&133));
        assert!(rx.try_iter().next().is_none());

        // An emptied list clears its bit, so a re-learn seeds again.
        lists.unlearn(8071);
        assert_eq!(refill(&lists, &mut actions, &net, mask), 0);
    }
}
