//! What the stock realm list reads off the install: the client Region's realm categories and the
//! realm types. Not [`crate::realmlist`], which is the logon server's address (the `realmList`
//! CVar). The list itself, its choice and Change Realm are the stock `RealmList` and its verbs
//! ([`crate::ui_script::glue_mgr`]).

use bevy::prelude::*;

use benilla_formats::{RealmCategory, RealmConfig};

/// The persisted CVar naming the last realm connected to; the SavedVariables path is built from it.
pub(crate) const CVAR_REALM_NAME: &str = "realmName";

/// The install's realm categories, read once at startup; `None` until then, and every realm lists
/// untabbed.
#[derive(Resource, Default)]
pub(crate) struct Realms {
    categories: Option<Vec<RealmCategory>>,
    configs: Option<Vec<RealmConfig>>,
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
        let types = self
            .configs
            .iter()
            .flatten()
            .map(|config| benilla_ui::script::GlueRealmType {
                realm_type: config.realm_type,
                pvp: config.pvp,
                rp: config.rp,
            })
            .collect();
        (categories, types)
    }

    /// The first matching row's PvP and RP flags, false when the type has no row.
    pub(crate) fn pvp_rp(&self, realm_type: u32) -> (bool, bool) {
        self.configs
            .iter()
            .flatten()
            .find(|r| r.realm_type == realm_type)
            .map_or((false, false), |r| (r.pvp, r.rp))
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
            configs: Some(vec![
                RealmConfig {
                    realm_type: 0,
                    pvp: false,
                    rp: false,
                },
                RealmConfig {
                    realm_type: 1,
                    pvp: true,
                    rp: false,
                },
                RealmConfig {
                    realm_type: 6,
                    pvp: false,
                    rp: true,
                },
                RealmConfig {
                    realm_type: 8,
                    pvp: true,
                    rp: true,
                },
                RealmConfig {
                    realm_type: 10,
                    pvp: true,
                    rp: false,
                },
            ]),
        }
    }
}

/// The install's realm categories and type configurations, read once.
pub(crate) struct RealmSelectPlugin;

/// The realm-table load, for a reader of [`Realms`] to order itself after.
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RealmSelectSet;

impl Plugin for RealmSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Realms>().add_systems(
            Startup,
            load_realm_tables
                .after(benilla_assets::AssetSet::Open)
                .in_set(RealmSelectSet),
        );
    }
}

/// Read both realm tables once, after the archive chain opens.
fn load_realm_tables(mut realms: ResMut<Realms>, assets: Option<Res<benilla_assets::WorldAssets>>) {
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
    match benilla_formats::load_realm_configs(&mut chain) {
        Ok(configs) => {
            info!("realm list: {} realm configurations", configs.len());
            realms.configs = Some(configs);
        }
        Err(e) => warn!("realm list: Cfg_Configs.dbc failed to load: {e:#}"),
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
            ..Default::default()
        };
        let (categories, _) = realms.glue_setup();
        let names: Vec<&str> = categories.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"United States"), "{names:?}");
    }

    /// Both the glue list and a selected realm use the same install rows.
    #[test]
    fn the_realm_types_are_the_shipped_cfg_configs_rows() {
        let data = benilla_formats::wow_data_or_skip!();
        let mut chain = benilla_formats::open_chain(&data).expect("open chain");
        let configs = benilla_formats::load_realm_configs(&mut chain).expect("Cfg_Configs.dbc");
        let realms = Realms {
            configs: Some(configs.clone()),
            ..Default::default()
        };
        let (_, glue) = realms.glue_setup();
        assert_eq!(glue.len(), configs.len());
        for (row, entry) in configs.iter().zip(glue) {
            assert_eq!(
                (entry.realm_type, entry.pvp, entry.rp),
                (row.realm_type, row.pvp, row.rp)
            );
            assert_eq!(realms.pvp_rp(row.realm_type), (row.pvp, row.rp));
        }
        assert_eq!(realms.pvp_rp(77), (false, false));
    }
}
