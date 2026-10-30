//! Character appearance render data: the geosets an appearance shows ([`CharacterGeosets`]) and the
//! body skin it wears: body-skin batches (`M2TextureType::Other(1)`) have no texture of their own,
//! and [`CharSections::composite_body`] builds one on the 256² partition and the section→cell map.
//!
//! DBC layouts follow the client's record readers: CharSections, the geoset tables. The compositor
//! disables geosets 0..=1700; higher IDs keep their model visibility through
//! [`VisibleGeosets::contains`].

mod components;
mod customization;
mod geosets;
mod sections;
mod shipped;

pub use components::{ComponentContext, ComponentLook};
pub use customization::{
    CharCreateCatalog, CharSectionGrid, CreateRaceRow, NameGenRow, StartOutfitItem,
};
pub use geosets::{CharacterGeosets, EquipGeosets, VisibleGeosets};
pub use sections::{
    equip_blits, equip_column, equip_region_candidates, equip_tex_dir, equip_tile, forearm_dressed,
    BlitSource, CharSections, CompositePlan, EmblemLayer, EquipBlit, GuildEmblem,
};

// The loaders' own schemas, for the `benilla-extract dbc` CSV dump (`crate::schema_for`) and the
// other readers of the same table.
pub(crate) use customization::{chr_races_schema, name_gen_schema, CHAR_BASE_INFO_WIDTH};
pub(crate) use geosets::{char_facial_hair_schema, char_hair_geosets_schema, helmet_vis_schema};
pub(crate) use sections::char_sections_schema;
