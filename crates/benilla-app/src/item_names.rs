//! The item-name cache, 12340's `itemnamecache.wdb` (`0xc5d7a0`): an entry's name and
//! `InventoryType`, asked by `CMSG_ITEM_NAME_QUERY` and answered by
//! `SMSG_ITEM_NAME_QUERY_RESPONSE` (`0x6351d0`). Its one reader is the item tooltip's set block,
//! which names the set's pieces from it (`0x62ba33`).

use bevy::prelude::*;

use benilla_protocol::{SessionEvent, SessionEventKind};

use crate::net::{ClientCommand, NetCommands, NetHandlerApp};
use crate::query_cache::QueryCache;

/// One entry's answer.
pub(crate) struct ItemName {
    pub(crate) name: String,
    pub(crate) inventory_type: u32,
}

#[derive(Resource, Default)]
pub(crate) struct ItemNames {
    names: QueryCache<u32, ItemName>,
}

/// `itemnamecache.wdb`.
impl crate::wdb::Persisted for ItemNames {
    fn drop_persisted(&mut self) {
        self.names.clear();
    }
}

impl crate::query_cache::AskOnce for ItemNames {
    fn clear_pending(&mut self) {
        self.names.clear_pending();
    }
}

impl ItemNames {
    /// The entry's answer, asked once while it is missing.
    pub(crate) fn get_or_ask(&self, entry: u32, commands: &NetCommands) -> Option<&ItemName> {
        self.names.get_or_ask(entry, || {
            debug!("item names: asking {entry}");
            let _ = commands.0.send(ClientCommand::ItemNameQuery { entry });
        })
    }

    /// Whether the entry is answered, known or not.
    pub(crate) fn answered(&self, entry: u32) -> bool {
        self.names.answered(entry)
    }
}

/// `SMSG_ITEM_NAME_QUERY_RESPONSE`: an answer lands, an unknown entry answers `None`.
fn on_item_name(In(ev): In<SessionEvent>, mut names: ResMut<ItemNames>) {
    if let SessionEvent::ItemName { entry, name } = ev {
        debug!("item names: {entry} answered: {name:?}");
        names.names.insert(
            entry,
            name.map(|(name, inventory_type)| ItemName {
                name,
                inventory_type,
            }),
        );
    }
}

pub(crate) fn register(app: &mut App) {
    crate::query_cache::register::<ItemNames>(app);
    app.init_resource::<ItemNames>()
        .net_handler(SessionEventKind::ItemName, on_item_name);
}
