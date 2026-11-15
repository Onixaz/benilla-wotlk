//! A 32-bit PE executable laid out as the loader maps it, read from disk: the headers at the image
//! base and each section at its virtual address, the bytes past a section's file data zero. The
//! file is read once and never written or run.

use std::path::Path;

use anyhow::{bail, ensure, Context, Result};

/// One span of the mapped image: where it starts, how long it is mapped, and its file bytes.
struct Span {
    va: u32,
    size: u32,
    bytes: Vec<u8>,
}

/// An executable's image as mapped at its preferred base, which a non-relocated load keeps.
pub struct ExeImage {
    image_base: u32,
    spans: Vec<Span>,
}

impl ExeImage {
    /// Read and map `path`.
    pub fn open(path: &Path) -> Result<Self> {
        let file = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::parse(&file).with_context(|| format!("mapping {}", path.display()))
    }

    /// Map a PE file's bytes: the DOS stub's `e_lfanew`, the `PE\0\0` signature, the file header's
    /// section count, the optional header's image base and header size, then the section table.
    pub fn parse(file: &[u8]) -> Result<Self> {
        let u16_at = |at: usize| -> Result<u16> {
            Ok(u16::from_le_bytes(
                file.get(at..at + 2).context("short PE")?.try_into()?,
            ))
        };
        let u32_at = |at: usize| -> Result<u32> {
            Ok(u32::from_le_bytes(
                file.get(at..at + 4).context("short PE")?.try_into()?,
            ))
        };
        ensure!(file.starts_with(b"MZ"), "no MZ header");
        let pe = u32_at(0x3c)? as usize;
        ensure!(
            file.get(pe..pe + 4) == Some(b"PE\0\0".as_slice()),
            "no PE signature"
        );
        let sections = usize::from(u16_at(pe + 6)?);
        let optional_len = usize::from(u16_at(pe + 20)?);
        let optional = pe + 24;
        // The PE32 magic; a PE32+ image keeps a 64-bit base at another offset.
        if u16_at(optional)? != 0x10b {
            bail!("not a 32-bit PE image");
        }
        let image_base = u32_at(optional + 28)?;
        let headers_len = u32_at(optional + 60)?;
        let mut spans = vec![Span {
            va: image_base,
            size: headers_len,
            bytes: file
                .get(..(headers_len as usize).min(file.len()))
                .unwrap_or_default()
                .to_vec(),
        }];
        let table = optional + optional_len;
        for i in 0..sections {
            let at = table + 40 * i;
            let virtual_size = u32_at(at + 8)?;
            let rva = u32_at(at + 12)?;
            let raw_size = u32_at(at + 16)? as usize;
            let raw_at = u32_at(at + 20)? as usize;
            let bytes = file
                .get(raw_at..(raw_at + raw_size).min(file.len()))
                .unwrap_or_default()
                .to_vec();
            spans.push(Span {
                va: image_base.wrapping_add(rva),
                size: virtual_size.max(raw_size as u32),
                bytes,
            });
        }
        Ok(Self { image_base, spans })
    }

    /// The preferred load address.
    pub fn image_base(&self) -> u32 {
        self.image_base
    }

    /// `len` bytes at virtual address `va`, or `None` when they leave the span they start in, as a
    /// read past a mapped region faults. Bytes the file does not carry read zero.
    pub fn read(&self, va: u32, len: usize) -> Option<Vec<u8>> {
        let span = self
            .spans
            .iter()
            .find(|s| va >= s.va && u64::from(va) < u64::from(s.va) + u64::from(s.size))?;
        let start = (va - span.va) as usize;
        if start + len > span.size as usize {
            return None;
        }
        Some(
            (start..start + len)
                .map(|i| span.bytes.get(i).copied().unwrap_or(0))
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal image: headers of 0x200 bytes at base 0x400000, one section at RVA 0x1000 of
    /// 0x20 virtual bytes, 0x10 of them in the file at 0x200.
    fn tiny() -> Vec<u8> {
        let mut f = vec![0u8; 0x210];
        f[..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        f[0x40..0x44].copy_from_slice(b"PE\0\0");
        f[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        f[0x54..0x56].copy_from_slice(&0xe0u16.to_le_bytes());
        let opt = 0x58;
        f[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
        f[opt + 28..opt + 32].copy_from_slice(&0x40_0000u32.to_le_bytes());
        f[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
        let sec = opt + 0xe0;
        f[sec..sec + 5].copy_from_slice(b".text");
        f[sec + 8..sec + 12].copy_from_slice(&0x20u32.to_le_bytes());
        f[sec + 12..sec + 16].copy_from_slice(&0x1000u32.to_le_bytes());
        f[sec + 16..sec + 20].copy_from_slice(&0x10u32.to_le_bytes());
        f[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
        for (i, b) in f[0x200..0x210].iter_mut().enumerate() {
            *b = i as u8 + 1;
        }
        f
    }

    #[test]
    fn a_section_reads_at_its_virtual_address_and_zero_past_its_file_bytes() {
        let image = ExeImage::parse(&tiny()).unwrap();
        assert_eq!(image.image_base(), 0x40_0000);
        assert_eq!(image.read(0x40_0000, 2).unwrap(), b"MZ");
        assert_eq!(image.read(0x40_1000, 3).unwrap(), [1, 2, 3]);
        assert_eq!(image.read(0x40_100e, 4).unwrap(), [15, 16, 0, 0]);
        assert_eq!(image.read(0x40_101f, 2), None, "past the section's end");
        assert_eq!(image.read(0x50_0000, 1), None, "nothing mapped there");
    }

    /// The install's executable maps at its own base with its headers there.
    #[test]
    fn the_installs_executable_maps_its_headers_at_its_base() {
        let exe = crate::wow_exe_or_skip!();
        let image = ExeImage::open(&exe).unwrap();
        assert_eq!(
            image.read(image.image_base(), 2).as_deref(),
            Some(b"MZ".as_slice())
        );
    }
}
