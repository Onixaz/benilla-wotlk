//! The chain's mount order, then the archive each named path resolves to.
//! `cargo run -p benilla-formats --example which_archive -- 'Fonts\FRIZQT__.TTF'`

fn main() -> anyhow::Result<()> {
    let data = benilla_formats::wow_data().expect("no WoW install found (set $WOW_DATA)");
    let chain = benilla_formats::open_chain(&data)?;
    println!("3.3.5a (12340) — lowest priority first:");
    for archive in chain.archive_paths() {
        println!("  {}", archive.display());
    }
    for name in std::env::args().skip(1) {
        match chain.find_file_archive(&name) {
            Some(archive) if chain.contains(&name) => println!("{name}: {}", archive.display()),
            Some(archive) => println!("{name}: deleted by {}", archive.display()),
            None => println!("{name}: not in the chain"),
        }
    }
    Ok(())
}
