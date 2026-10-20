//! Skin profiles (`<Model>NN.skin`): indices, triangles, submeshes and batches.

use benilla_bytes::{capped, ByteExt};

use crate::error::{Error, Result};

/// One skin submesh (`M2SkinSection`, `0x30` bytes).
pub struct SkinSection {
    /// `skinSectionId`'s low half, the geoset id `group*100 + variant` that character geoset
    /// selection toggles; 0 on a single-geoset model.
    pub id: u16,
    pub vertex_start: u16,
    pub vertex_count: u16,
    /// The triangle range, read as the `u16`s the draw takes (`0x820b9c`).
    pub triangle_start: u16,
    pub triangle_count: u16,
    /// The bone whose matrix carries [`Self::sort_center`] (`+0x12`).
    pub center_bone: u16,
    /// The section's centre (`+0x14`), model space.
    pub center: [f32; 3],
    /// The point the transparent sort measures from (`+0x20`), model space.
    pub sort_center: [f32; 3],
    /// The radius the transparent sort pushes that point by (`+0x2c`).
    pub sort_radius: f32,
}

/// One skin draw batch (`M2Batch`: section + material + texture-combo references).
pub struct SkinBatch {
    /// The batch flags (`+0x00`) and, in the high byte, the priority plane (`+0x01`).
    pub flags: u16,
    /// The shader word (`+0x02`): under global flag `0x8`, an index into the combiner ops.
    pub shader_id: u16,
    pub skin_section_index: u16,
    pub texture_combo_index: u16,
    /// Index into [`crate::M2Model::texture_unit_lookup`], the stage's coordinate source
    /// (`+0x12`).
    pub texture_coord_combo_index: u16,
    pub material_index: u16,
    /// The batch's layer over the base batch of its section (`+0x0c`), 0 for the base.
    pub material_layer: u16,
    /// Index into [`crate::M2Model::color_alpha_tracks`], `0xffff` for none (`+0x08`).
    pub color_index: u16,
    /// Index into [`crate::M2Model::transparency_lookup`] (`+0x14`).
    pub weight_combo_index: u16,
    /// Texture count (`+0x0e`); at 0 the transparency factor is not applied.
    pub texture_count: u16,
    /// Index into [`crate::M2Model::texture_transform_lookup`] (`+0x16`).
    pub texture_transform_combo_index: u16,
}

impl SkinBatch {
    /// The priority plane, the transparent sort's key within one sort depth (`+0x01`, `i8`).
    pub fn priority_plane(&self) -> i8 {
        self.flags.to_le_bytes()[1] as i8
    }
}

/// A decoded skin profile.
#[derive(Default)]
pub struct Skin {
    indices: Vec<u16>,
    triangles: Vec<u16>,
    submeshes: Vec<SkinSection>,
    batches: Vec<SkinBatch>,
}
impl Skin {
    pub fn indices(&self) -> &Vec<u16> {
        &self.indices
    }
    pub fn triangles(&self) -> &Vec<u16> {
        &self.triangles
    }
    pub fn submeshes(&self) -> &Vec<SkinSection> {
        &self.submeshes
    }
    pub fn batches(&self) -> &Vec<SkinBatch> {
        &self.batches
    }

    /// Decode a `.skin` file: the `SKIN` magic, then vertex indices `+0x04`, triangles `+0x0c`,
    /// bone properties `+0x14`, sections `+0x1c` and batches `+0x24`, each an `M2Array` into the
    /// file.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.get(0..4) != Some(&b"SKIN"[..]) {
            return Err(Error::NotSkin);
        }
        let rd_arr = |o: usize| -> Result<(usize, usize)> {
            Ok((
                bytes.u32_at(o).ok_or(Error::Truncated)? as usize,
                bytes.u32_at(o + 4).ok_or(Error::Truncated)? as usize,
            ))
        };
        let (n_idx, o_idx) = rd_arr(0x04)?;
        let (n_tri, o_tri) = rd_arr(0x0c)?;
        let (n_sub, o_sub) = rd_arr(0x1c)?;
        let (n_bat, o_bat) = rd_arr(0x24)?;

        // `bytes_at` checks the whole run first, so `n` needs no `capped`.
        let read_u16s = |n: usize, o: usize| -> Result<Vec<u16>> {
            let len = n.checked_mul(2).ok_or(Error::Truncated)?;
            let slice = bytes.bytes_at(o, len).ok_or(Error::Truncated)?;
            Ok(slice
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect())
        };
        let indices = read_u16s(n_idx, o_idx)?;
        let triangles = read_u16s(n_tri, o_tri)?;

        let vec3 = |s: &[u8], o: usize| -> Result<[f32; 3]> {
            Ok([
                s.f32_at(o).ok_or(Error::Truncated)?,
                s.f32_at(o + 4).ok_or(Error::Truncated)?,
                s.f32_at(o + 8).ok_or(Error::Truncated)?,
            ])
        };
        // `n_sub` and `n_bat` are unvalidated, so the reservations are capped.
        let sub_avail = bytes.len().saturating_sub(o_sub);
        let mut submeshes = Vec::with_capacity(capped(n_sub, 0x30, sub_avail));
        for i in 0..n_sub {
            let s = bytes
                .bytes_at(o_sub + i * 0x30, 0x30)
                .ok_or(Error::Truncated)?;
            let u = |o: usize| s.u16_at(o).ok_or(Error::Truncated);
            submeshes.push(SkinSection {
                id: u(0)?,
                vertex_start: u(4)?,
                vertex_count: u(6)?,
                triangle_start: u(8)?,
                triangle_count: u(10)?,
                center_bone: u(0x12)?,
                center: vec3(s, 0x14)?,
                sort_center: vec3(s, 0x20)?,
                sort_radius: s.f32_at(0x2c).ok_or(Error::Truncated)?,
            });
        }
        let bat_avail = bytes.len().saturating_sub(o_bat);
        let mut batches = Vec::with_capacity(capped(n_bat, 0x18, bat_avail));
        for i in 0..n_bat {
            let b = bytes
                .bytes_at(o_bat + i * 0x18, 0x18)
                .ok_or(Error::Truncated)?;
            let u = |o: usize| b.u16_at(o).ok_or(Error::Truncated);
            batches.push(SkinBatch {
                flags: u(0)?,
                shader_id: u(2)?,
                skin_section_index: u(4)?,
                color_index: u(0x08)?,
                material_index: u(0x0a)?,
                material_layer: u(0x0c)?,
                texture_count: u(0x0e)?,
                texture_combo_index: u(0x10)?,
                texture_coord_combo_index: u(0x12)?,
                weight_combo_index: u(0x14)?,
                texture_transform_combo_index: u(0x16)?,
            });
        }
        Ok(Self {
            indices,
            triangles,
            submeshes,
            batches,
        })
    }
}
