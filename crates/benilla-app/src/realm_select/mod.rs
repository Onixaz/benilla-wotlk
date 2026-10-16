//! What the stock realm list reads off the install: the client Region's realm categories and the
//! realm types. Not [`crate::realmlist`], which is the logon server's address (the `realmList`
//! CVar). The list itself, its choice and Change Realm are the stock `RealmList` and its verbs
//! ([`crate::ui_script::glue_mgr`]).

use bevy::prelude::*;

use benilla_formats::RealmCategory;

/// The persisted CVar naming the last realm connected to; the SavedVariables path is built from it.
pub(crate) const CVAR_REALM_NAME: &str = "realmName";

/// The install's realm categories, read once at startup; `None` until then, and every realm lists
/// untabbed.
#[derive(Resource, Default)]
pub(crate) struct Realms {
    categories: Option<Vec<RealmCategory>>,
}

impl Realms {
    /// The client Region's categories and the realm types, as the stock glue's realm verbs read
    /// them; no categories until the DBC is read.
    pub(crate) fn glue_setup(
        &self,
    ) -> (
        Vec<benilla_ui::script::GlueRealmCategory>,
        Vec<benilla_ui::script::GlueRealmType>,
    ) {
        let categories = self
            .categories
            .iter()
            .flatten()
            .map(|c| benilla_ui::script::GlueRealmCategory {
                id: c.id,
                name: c.name.clone(),
                locale_mask: c.locale_mask,
                flags: c.flags,
            })
            .collect();
        let types = REALM_TYPES
            .map(|realm_type| {
                let (pvp, rp) = pvp_rp(realm_type);
                benilla_ui::script::GlueRealmType {
                    realm_type,
                    pvp,
                    rp,
                }
            })
            .collect();
        (categories, types)
    }

    /// A realm type's `(pvp, rp)` in `Cfg_Configs.dbc`, `(false, false)` for a type with no row.
    pub(crate) fn pvp_rp(&self, realm_type: u32) -> (bool, bool) {
        pvp_rp(realm_type)
    }
}

#[cfg(test)]
impl Realms {
    /// A US client's state at startup, read without an install: Region 1's two categories.
    pub(crate) fn us_client() -> Realms {
        let cat = |id: u32, name: &str| RealmCategory {
            locale_mask: 0,
            flags: 0,
            id,
            name: name.into(),
        };
        Realms {
            categories: Some(vec![cat(1, "United States"), cat(5, "Oceanic")]),
        }
    }
}

/// The realm types `Cfg_Configs.dbc` has a row for.
const REALM_TYPES: std::ops::RangeInclusive<u32> = 0..=8;

/// The realm type as `(pvp, rp)`, shared by `GetRealmInfo` and `GetServerName`: the
/// `Cfg_Configs.dbc` row whose `RealmType` matches, its `PlayerKillingAllowed` and
/// `RoleplayingRealm` transcribed (types 3 and 5 are PvP, 7 is RP); a type with no row reads
/// neither.
pub(crate) fn pvp_rp(realm_type: u32) -> (bool, bool) {
    match realm_type {
        0 | 2 | 4 => (false, false),
        1 | 3 | 5 => (true, false),
        6 | 7 => (false, true),
        8 => (true, true),
        _ => (false, false),
    }
}

/// The install's realm categories, read once.
pub(crate) struct RealmSelectPlugin;

/// The realm-category load, for a reader of [`Realms`] to order itself after.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RealmSelectSet;

impl Plugin for RealmSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Realms>().add_systems(
            Startup,
            load_categories
                .after(benilla_assets::AssetSet::Open)
                .in_set(RealmSelectSet),
        );
    }
}

/// Read the category set once, after the archive chain opens.
fn load_categories(mut realms: ResMut<Realms>, assets: Option<Res<benilla_assets::WorldAssets>>) {
    use benilla_assets::LockRecover;
    let Some(assets) = assets else {
        return;
    };
    let mut chain = assets.chain.lock_recover();
    match benilla_formats::load_realm_categories(&mut chain) {
        Ok(categories) => {
            info!("realm list: {} categories", categories.len());
            if categories.is_empty() {
                warn!(
                    "realm list: Cfg_Categories.dbc has no rows, so every realm lists on one \
                     untabbed page"
                );
            }
            realms.categories = Some(categories);
        }
        Err(e) => warn!(
            "realm list: Cfg_Categories.dbc failed to load, so every realm lists on one untabbed \
             page: {e:#}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The install's own `Cfg_Categories.dbc` names the categories the stock list tabs on.
    #[test]
    fn the_categories_come_from_the_install() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let loaded = benilla_formats::load_realm_categories(&mut chain).expect("DBC");
        let realms = Realms {
            categories: Some(loaded),
        };
        let (categories, _) = realms.glue_setup();
        let names: Vec<&str> = categories.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"United States"), "{names:?}");
    }

    /// The types are the shipped `Cfg_Configs` rows; a type with no row reads neither column.
    #[test]
    fn the_realm_types_are_the_shipped_cfg_configs_rows() {
        for t in [0, 2, 4] {
            assert_eq!(pvp_rp(t), (false, false), "type {t}");
        }
        for t in [1, 3, 5] {
            assert_eq!(pvp_rp(t), (true, false), "type {t}");
        }
        for t in [6, 7] {
            assert_eq!(pvp_rp(t), (false, true), "type {t}");
        }
        assert_eq!(pvp_rp(8), (true, true));
        assert_eq!(pvp_rp(77), (false, false));
    }
}
