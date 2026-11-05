//! `Movie.dbc` and the three tables that pick its file: `MovieVariation.dbc` (a movie's files),
//! `MovieFileData.dbc` (each file's resolution) and `FileData.dbc` (each file's name and path).
//! `StartMovie` (`0x5235b0`) plays a row only when one of its files resolves
//! (`FindMovieFileDataID`, `0x518700`).

use std::collections::HashMap;

use anyhow::{Context, Result};
use benilla_dbc::{FieldType, Schema, SchemaField};

use crate::dbc::{parse, str_at, u32_at};
use crate::Chain;

const MOVIE: &str = "DBFilesClient\\Movie.dbc";
const MOVIE_VARIATION: &str = "DBFilesClient\\MovieVariation.dbc";
const MOVIE_FILE_DATA: &str = "DBFilesClient\\MovieFileData.dbc";
const FILE_DATA: &str = "DBFilesClient\\FileData.dbc";

/// A file of a movie, as `FileData.dbc` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovieFile {
    pub file_data_id: u32,
    /// `Filepath` then `Filename`, the extension cut at the last `.` (`0x51d749`): the name
    /// `PLAY_MOVIE` carries.
    pub name: String,
}

/// The four tables, keyed as the reference looks them up.
#[derive(Debug, Clone, Default)]
pub struct MovieCatalog {
    /// `Movie.dbc`: id to its `Volume` (`[row+8]`).
    volumes: HashMap<u32, u32>,
    /// `MovieVariation.dbc` in file order: `(MovieID, FileDataID)`.
    variations: Vec<(u32, u32)>,
    /// `MovieFileData.dbc`: file to its horizontal resolution.
    resolutions: HashMap<u32, u32>,
    /// `FileData.dbc`: file to `(Filename, Filepath)`.
    files: HashMap<u32, (String, String)>,
}

impl MovieCatalog {
    /// Build from rows, for tests: movies as `(id, volume)`, variations as `(movie, file)`,
    /// resolutions as `(file, width)`, files as `(id, name, path)`.
    pub fn from_rows(
        movies: &[(u32, u32)],
        variations: &[(u32, u32)],
        resolutions: &[(u32, u32)],
        files: &[(u32, &str, &str)],
    ) -> Self {
        Self {
            volumes: movies.iter().copied().collect(),
            variations: variations.to_vec(),
            resolutions: resolutions.iter().copied().collect(),
            files: files
                .iter()
                .map(|&(id, name, path)| (id, (name.to_string(), path.to_string())))
                .collect(),
        }
    }

    /// The movie row's volume, `None` without a row (`0x5235be`).
    pub fn volume(&self, movie: u32) -> Option<u32> {
        self.volumes.get(&movie).copied()
    }

    /// The movie's file for a screen `width` pixels wide (`0x518700`): among the variations whose
    /// file has both a resolution and a name, an exact width wins at once; otherwise the pick
    /// moves down while it is wider than the screen and up while it stays below it.
    pub fn file_for(&self, movie: u32, width: u32) -> Option<MovieFile> {
        self.volumes.get(&movie)?;
        let (width, mut best, mut best_res) = (width as i64, None, 0i64);
        for &(m, file) in &self.variations {
            if m != movie {
                continue;
            }
            let (Some(&res), Some(_)) = (self.resolutions.get(&file), self.files.get(&file)) else {
                continue;
            };
            let res = i64::from(res);
            if res == width {
                best = Some(file);
                break;
            }
            let ceiling = if width <= best_res { best_res } else { width };
            let better = best.is_none() || ((width <= best_res || best_res < res) && res < ceiling);
            if better {
                best = Some(file);
                best_res = res;
            }
        }
        let file = best?;
        let (name, path) = self.files.get(&file)?;
        let mut full = format!("{path}{name}");
        if let Some(dot) = full.rfind('.') {
            full.truncate(dot);
        }
        Some(MovieFile {
            file_data_id: file,
            name: full,
        })
    }
}

pub(crate) fn movie_schema() -> Schema {
    let mut s = Schema::new("Movie");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Filename", FieldType::String));
    s.add_field(SchemaField::new("Volume", FieldType::UInt32));
    s
}

pub(crate) fn movie_variation_schema() -> Schema {
    let mut s = Schema::new("MovieVariation");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("MovieID", FieldType::UInt32));
    s.add_field(SchemaField::new("FileDataID", FieldType::UInt32));
    s
}

pub(crate) fn movie_file_data_schema() -> Schema {
    let mut s = Schema::new("MovieFileData");
    s.add_field(SchemaField::new("FileDataID", FieldType::UInt32));
    s.add_field(SchemaField::new("Resolution", FieldType::UInt32));
    s
}

pub(crate) fn file_data_schema() -> Schema {
    let mut s = Schema::new("FileData");
    s.add_field(SchemaField::new("ID", FieldType::UInt32));
    s.add_field(SchemaField::new("Filename", FieldType::String));
    s.add_field(SchemaField::new("Filepath", FieldType::String));
    s
}

/// Load the four tables from the patch chain.
pub fn load_movies(chain: &mut Chain) -> Result<MovieCatalog> {
    let mut read = |path: &str, schema: Schema, what: &str| -> Result<_> {
        let bytes = chain
            .read_file(path)
            .with_context(|| format!("reading {path}"))?;
        parse(&bytes, schema, what)
    };
    let movies = read(MOVIE, movie_schema(), "Movie")?;
    let variations = read(MOVIE_VARIATION, movie_variation_schema(), "MovieVariation")?;
    let resolutions = read(MOVIE_FILE_DATA, movie_file_data_schema(), "MovieFileData")?;
    let files = read(FILE_DATA, file_data_schema(), "FileData")?;
    Ok(MovieCatalog {
        volumes: movies
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 2)?)))
            .collect(),
        variations: variations
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 1)?, u32_at(r, 2)?)))
            .collect(),
        resolutions: resolutions
            .records()
            .iter()
            .filter_map(|r| Some((u32_at(r, 0)?, u32_at(r, 1)?)))
            .collect(),
        files: files
            .records()
            .iter()
            .filter_map(|r| {
                let id = u32_at(r, 0)?;
                Some((
                    id,
                    (
                        str_at(&files, r, 1).unwrap_or_default(),
                        str_at(&files, r, 2).unwrap_or_default(),
                    ),
                ))
            })
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> MovieCatalog {
        MovieCatalog::from_rows(
            &[(2, 120)],
            &[(2, 10), (2, 11), (2, 12)],
            &[(10, 800), (11, 1024), (12, 1280)],
            &[
                (10, "WotLK_800.avi", "Interface\\Cinematics\\"),
                (11, "WotLK_1024.avi", "Interface\\Cinematics\\"),
                (12, "WotLK_1280.avi", "Interface\\Cinematics\\"),
            ],
        )
    }

    /// `0x518700`: an exact width wins, else the widest file below the screen, else the narrowest
    /// above it; the name drops its extension.
    #[test]
    fn the_file_follows_the_screen_width() {
        let c = catalog();
        assert_eq!(c.file_for(2, 1024).unwrap().file_data_id, 11);
        assert_eq!(c.file_for(2, 1200).unwrap().file_data_id, 11);
        assert_eq!(c.file_for(2, 1920).unwrap().file_data_id, 12);
        assert_eq!(c.file_for(2, 640).unwrap().file_data_id, 10);
        assert_eq!(
            c.file_for(2, 1280).unwrap().name,
            "Interface\\Cinematics\\WotLK_1280"
        );
        assert_eq!(c.file_for(3, 1024), None, "no Movie.dbc row");
        assert_eq!(c.volume(2), Some(120));
    }

    #[test]
    fn the_shipped_tables_name_a_file_for_every_movie_they_vary() {
        let data = crate::wow_data_or_skip!();
        let mut chain = crate::open_chain(&data).expect("open chain");
        let c = load_movies(&mut chain).expect("movie tables");
        assert!(!c.volumes.is_empty() && !c.variations.is_empty());
        for &(movie, _) in &c.variations {
            assert!(c.file_for(movie, 1024).is_some(), "movie {movie}");
        }
    }
}
