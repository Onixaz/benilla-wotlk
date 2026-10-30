//! The item-set block's count (`0x6276e0` per worn piece): which members the player wears, how
//! many pieces of the set are worn, and how many slots the set fills.

use crate::script::ItemSetMember;

/// A worn item's template, as the count reads it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct WornPiece {
    pub id: u32,
    pub name: String,
    pub item_set: u32,
    pub inventory_type: u32,
}

/// The set block's numbers.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct SetCount {
    /// Per member, in DBC order, the name of the worn piece standing for it.
    pub members: Vec<Option<String>>,
    /// The worn pieces carrying the set's id, the header's first count and the bonus gate.
    pub worn: usize,
    /// The slots the set fills, the header's second count.
    pub total: usize,
}

/// Chest (5) and robe (20) are one slot.
fn same_slot(a: u32, b: u32) -> bool {
    a == b || matches!((a, b), (5, 20) | (20, 5))
}

/// Count `worn` against the set: a worn piece counts when its own template names the set, so a
/// variant outside the DBC's list counts too. The first pass binds members by id; the second
/// binds the rest by slot type and stands the worn piece's name in for the member's.
pub(super) fn count_set(set_id: u32, members: &[ItemSetMember], worn: &[WornPiece]) -> SetCount {
    let mut bound: Vec<Option<&WornPiece>> = vec![None; members.len()];
    let mut used = vec![false; worn.len()];
    let pieces: Vec<(usize, &WornPiece)> = worn
        .iter()
        .enumerate()
        .filter(|(_, p)| p.item_set == set_id)
        .collect();
    for &(at, piece) in &pieces {
        if let Some(i) =
            (0..members.len()).find(|&i| bound[i].is_none() && members[i].id == piece.id)
        {
            bound[i] = Some(piece);
            used[at] = true;
        }
    }
    for &(_, piece) in pieces.iter().filter(|&&(at, _)| !used[at]) {
        if let Some(i) = (0..members.len()).find(|&i| {
            bound[i].is_none()
                && members[i].name.is_some()
                && same_slot(piece.inventory_type, members[i].inventory_type)
        }) {
            bound[i] = Some(piece);
        }
    }
    // A member counts toward the total when every member before it is resolved and none shares
    // its slot; rings, trinkets and one-handers may repeat.
    let total = (0..members.len())
        .filter(|&i| {
            let m = &members[i];
            m.name.is_some()
                && members[..i].iter().all(|p| {
                    p.name.is_some()
                        && !(same_slot(m.inventory_type, p.inventory_type)
                            && !(matches!(m.inventory_type, 11..=13)
                                && m.inventory_type == p.inventory_type))
                })
        })
        .count();
    SetCount {
        members: bound.iter().map(|b| b.map(|p| p.name.clone())).collect(),
        worn: pieces.len(),
        total,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(id: u32, name: &str, item_set: u32, inventory_type: u32) -> WornPiece {
        WornPiece {
            id,
            name: name.into(),
            item_set,
            inventory_type,
        }
    }

    fn tier() -> Vec<ItemSetMember> {
        vec![
            ItemSetMember::new(10, "Heroes' Tunic", 5),
            ItemSetMember::new(11, "Heroes' Handguards", 10),
            ItemSetMember::new(12, "Heroes' Headpiece", 1),
        ]
    }

    #[test]
    fn a_listed_piece_binds_its_own_member() {
        let c = count_set(794, &tier(), &[piece(11, "Heroes' Handguards", 794, 10)]);
        assert_eq!(c.worn, 1);
        assert_eq!(c.total, 3);
        assert_eq!(c.members, [None, Some("Heroes' Handguards".into()), None]);
    }

    #[test]
    fn a_variant_outside_the_list_counts_and_stands_in_by_slot() {
        // The 25-player pieces carry the set's id but no DBC member's.
        let worn = [
            piece(20, "Valorous Tunic", 794, 20),
            piece(21, "Valorous Handguards", 794, 10),
            piece(99, "Some Ring", 0, 11),
        ];
        let c = count_set(794, &tier(), &worn);
        assert_eq!(c.worn, 2, "both variants count, the ring does not");
        assert_eq!(
            c.members,
            [
                Some("Valorous Tunic".into()),
                Some("Valorous Handguards".into()),
                None
            ],
            "a robe stands in for the chest member"
        );
    }

    #[test]
    fn a_piece_with_no_free_member_still_counts() {
        let worn = [
            piece(10, "Heroes' Tunic", 794, 5),
            piece(20, "Valorous Tunic", 794, 5),
        ];
        let c = count_set(794, &tier(), &worn);
        assert_eq!(c.worn, 2);
        assert_eq!(c.members[0].as_deref(), Some("Heroes' Tunic"));
    }

    #[test]
    fn the_total_counts_slots_not_listed_items() {
        let members = vec![
            ItemSetMember::new(1, "Sword", 13),
            ItemSetMember::new(2, "Offhand Sword", 13),
            ItemSetMember::new(3, "Robe", 20),
            ItemSetMember::new(4, "Chest", 5),
            ItemSetMember::new(5, "Ring", 11),
            ItemSetMember::new(6, "Ring", 11),
        ];
        // One-handers and rings repeat; a robe and a chest are one slot.
        assert_eq!(count_set(1, &members, &[]).total, 5);
        let mut unresolved = members.clone();
        unresolved[0].name = None;
        assert_eq!(
            count_set(1, &unresolved, &[]).total,
            0,
            "a member after an unresolved one waits"
        );
    }
}
