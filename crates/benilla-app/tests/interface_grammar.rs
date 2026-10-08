//! Every Lua file of the stock interface on the player's chain compiles in the script host: the
//! VM's grammar is the reference client's, so a syntax error here is ours, never the file's.

use benilla_ui::script::UiScript;

/// The stock interface's Lua: FrameXML, GlueXML and Blizzard's own addons.
fn is_stock_interface_lua(name: &str) -> bool {
    let n = name.replace('/', "\\").to_ascii_lowercase();
    n.ends_with(".lua")
        && (n.starts_with("interface\\framexml\\")
            || n.starts_with("interface\\gluexml\\")
            || n.starts_with("interface\\addons\\blizzard_"))
}

#[test]
fn every_stock_interface_lua_file_compiles() {
    let data = benilla_formats::wow_data_or_skip!();
    let chain = benilla_formats::open_chain(&data).expect("the install's archive chain");
    let names: Vec<String> = chain
        .list()
        .expect("the chain's listfiles")
        .into_iter()
        .map(|e| e.name)
        .filter(|n| is_stock_interface_lua(n))
        .collect();
    assert!(
        names
            .iter()
            .any(|n| n.eq_ignore_ascii_case("Interface\\FrameXML\\UIParent.lua")),
        "the listfiles name the stock FrameXML"
    );

    let s = UiScript::new().unwrap();
    let errors: Vec<String> = names
        .iter()
        .filter_map(|name| {
            let bytes = chain.read(name).ok()?;
            s.lua()
                .load(benilla_ui::source::chunk(&bytes))
                .set_name(format!("@{name}"))
                .into_function()
                .err()
                .map(|e| e.to_string())
        })
        .collect();
    assert!(
        errors.is_empty(),
        "{} of {} stock interface files do not compile:\n{}",
        errors.len(),
        names.len(),
        errors.join("\n")
    );
}
