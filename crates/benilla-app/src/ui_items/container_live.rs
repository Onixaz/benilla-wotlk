//! The containers the bag verbs read at the call (`0x5d7380`): each bag built off the active player's slot fields, the item objects and the item cache when a verb asks, as 12340's verbs read the live bag.

use benilla_protocol::messages::CURRENCY_TOKEN_SLOTS;
use benilla_ui::script::{ContainerSlot, ContainerState};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;

use crate::entities::ItemDisplays;
use crate::items::Items;
use benilla_protocol::ObjectFields;

use crate::net::{NetCommands, Objects};
use crate::pending_item_ops::PendingItemOps;

use super::{
    has_key, BAG_SLOT_FIRST, BANK_BAG_ID_FIRST, BANK_BAG_ID_LAST, BANK_CONTAINER, BANK_SLOTS,
    KEYRING_CONTAINER, KEYRING_SLOTS, PACK_SLOTS,
};

/// The token container, `-4` (`0x5d7529`).
const TOKEN_CONTAINER: i64 = -4;

/// What a bag read takes beside the lent resources.
#[derive(SystemParam)]
pub(crate) struct ContainerLive<'w, 's> {
    objects: Objects<'w, 's>,
    items: Res<'w, Items>,
    icons: Option<Res<'w, ItemDisplays>>,
    enchants: Option<Res<'w, crate::items::Enchants>>,
    props: Option<Res<'w, crate::items::RandomProperties>>,
    names: Res<'w, crate::names::NameCache>,
    classes: Option<Res<'w, crate::chr_classes::ChrClassTable>>,
    spells: Option<Res<'w, crate::ui_action::Spells>>,
    commands: Res<'w, NetCommands>,
    watched: Option<Res<'w, super::watch::WatchedBags>>,
}

/// The lent state a bag read takes: the cooldown history, the item locks, the charter records and whether a banker is set (`0xbeb9a0`).
pub(crate) struct ContainerLent<'a> {
    pub(crate) cooldowns: Option<&'a crate::spell::Cooldowns>,
    pub(crate) pending: Option<&'a PendingItemOps>,
    pub(crate) petitions: Option<&'a crate::ui_petition::PetitionState>,
    pub(crate) banker: bool,
}

/// How a read names a bag: a Lua bag index resolves bags 1..11 through the watched-bag array behind the banker gate (`0x5d6f20`), a held-item walk reads the player's slot fields as the bag search does (`0x7546f0`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum BagRead {
    Index,
    Held,
}

/// A bag's shape: its name, size, family and the item guid in each slot.
struct Layout {
    name: Option<String>,
    num_slots: u32,
    bag_family: u32,
    guids: Vec<u64>,
}

impl ContainerLive<'_, '_> {
    /// The active player's fields, `None` with none held.
    fn me(&self) -> Option<&ObjectFields> {
        self.objects.player_object()
    }

    /// Bag `bag` (1..11) by Lua index (`0x5d6f20`): the watched bag, the bank's only with a banker set (`0x5d6f34`-`0x5d6f3f`).
    fn indexed_bag(&self, lent: &ContainerLent, bag: i64) -> Option<u64> {
        let i = usize::try_from(bag - 1).ok().filter(|&i| i < 11)?;
        if i >= 4 && !lent.banker {
            return None;
        }
        self.watched.as_ref().map(|w| w.0[i])
    }

    /// Container `bag`'s shape off the player's slot fields and the bag object (`0x5d7380`); `None` for an empty bag slot or an id past them.
    fn layout(
        &self,
        fields: &ObjectFields,
        lent: &ContainerLent,
        bag: i64,
        read: BagRead,
    ) -> Option<Layout> {
        let flat = |name: Option<&str>, n: u8, f: &dyn Fn(u8) -> Option<u64>| Layout {
            name: name.map(str::to_string),
            num_slots: u32::from(n),
            bag_family: 0,
            guids: (0..n).map(|i| f(i).unwrap_or(0)).collect(),
        };
        let bag_guid = match bag {
            0 => {
                return Some(flat(Some("Backpack"), PACK_SLOTS, &|i| {
                    fields.player_pack_slot(i)
                }))
            }
            BANK_CONTAINER => {
                return Some(flat(Some("Bank"), BANK_SLOTS, &|i| {
                    fields.player_bank_slot(i)
                }))
            }
            KEYRING_CONTAINER => {
                return Some(flat(Some("Keyring"), KEYRING_SLOTS, &|i| {
                    fields.player_keyring_slot(i)
                }))
            }
            TOKEN_CONTAINER => {
                return Some(flat(None, CURRENCY_TOKEN_SLOTS, &|i| {
                    fields.player_currency_token_slot(i)
                }))
            }
            1..=BANK_BAG_ID_LAST if read == BagRead::Index => self.indexed_bag(lent, bag),
            1..=4 => fields.player_inv_slot(BAG_SLOT_FIRST + bag as u8 - 1),
            BANK_BAG_ID_FIRST..=BANK_BAG_ID_LAST => {
                fields.player_bank_bag_slot((bag - BANK_BAG_ID_FIRST) as u8)
            }
            _ => None,
        }
        .filter(|&g| g != 0)?;
        let Some(object) = self.objects.object(bag_guid) else {
            return Some(Layout {
                name: None,
                num_slots: 0,
                bag_family: 0,
                guids: Vec::new(),
            });
        };
        let n = object.container_num_slots().unwrap_or(0);
        let entry = object.object_entry().unwrap_or(0);
        let (name, bag_family) = (entry != 0)
            .then(|| self.items.template(entry, bag_guid, &self.commands))
            .flatten()
            .map_or((None, 0), |t| (Some(t.name.clone()), t.bag_family));
        Some(Layout {
            name,
            num_slots: n,
            bag_family,
            guids: (0..n.min(36) as u8)
                .map(|j| object.container_slot(j).unwrap_or(0))
                .collect(),
        })
    }

    /// One slot's view; `None` for an empty one.
    fn resolve(
        &self,
        me: &ObjectFields,
        lent: &ContainerLent,
        bag: i64,
        guid: u64,
        (anchor, ui_now): (std::time::Instant, f64),
    ) -> Option<ContainerSlot> {
        let has_relic_slot = me.unit_class().is_some_and(|c| {
            self.classes
                .as_deref()
                .is_some_and(|t| t.0.has_relic_slot(u32::from(c)))
        });
        let mut view = super::feed::resolve_slot(
            guid,
            &self.objects,
            &self.items,
            self.icons.as_deref(),
            crate::items::RollCatalogs {
                enchants: self.enchants.as_deref(),
                props: self.props.as_deref(),
            },
            &self.commands,
            lent.cooldowns,
            self.spells.as_deref().map(|s| &s.catalog),
            &self.names,
            lent.petitions,
            anchor,
            ui_now,
            has_relic_slot,
        )?;
        // The token slots take no lock (`0x5d7529`).
        view.locked = bag != TOKEN_CONTAINER && lent.pending.is_some_and(|p| p.is_locked(guid));
        Some(view)
    }

    /// Container `bag` at the call: `None` with no active player, `Some(None)` for no such bag.
    pub(crate) fn bag(
        &self,
        lent: &ContainerLent,
        bag: i64,
        read: BagRead,
        clock: (std::time::Instant, f64),
    ) -> Option<Option<ContainerState>> {
        let me = self.me()?;
        let Some(layout) = self.layout(me, lent, bag, read) else {
            return Some(None);
        };
        let slots = layout
            .guids
            .iter()
            .enumerate()
            .filter_map(|(j, &guid)| {
                let slot = j as u32 + 1;
                self.resolve(me, lent, bag, guid, clock).map(|v| (slot, v))
            })
            .collect();
        Some(Some(ContainerState {
            name: layout.name,
            num_slots: layout.num_slots,
            slots,
            bag_family: layout.bag_family,
        }))
    }

    /// One 1-based slot of container `bag` at the call, as [`Self::bag`] reads it.
    pub(crate) fn slot(
        &self,
        lent: &ContainerLent,
        bag: i64,
        slot: u32,
        read: BagRead,
        clock: (std::time::Instant, f64),
    ) -> Option<Option<ContainerSlot>> {
        let me = self.me()?;
        let guid = slot
            .checked_sub(1)
            .and_then(|j| {
                self.layout(me, lent, bag, read)?
                    .guids
                    .get(j as usize)
                    .copied()
            })
            .unwrap_or(0);
        Some(self.resolve(me, lent, bag, guid, clock))
    }

    /// `HasKey()` at the call (`0x51ae80`): `None` with no active player.
    pub(crate) fn has_key(&self) -> Option<bool> {
        let me = self.me()?;
        Some(has_key(me, &self.objects, &self.items, &self.commands))
    }
}
