//! Shared fixtures for the script test modules.

use crate::script::UiScript;

pub(super) fn script() -> UiScript {
    let mut script = UiScript::new().expect("construct UiScript");
    script.set_paperdoll_item_frame(crate::script::PaperDollSlotInfo::test_rows());
    script
}
