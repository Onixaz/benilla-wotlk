//! The holders of the VM not yet converted to the `World` loan ([`super::safe_world`]): files whose
//! systems, handlers or `SystemParam` structs still hold `NonSend[Mut]<UiScript>` beside the
//! borrowed view (`LendLive`), with how many each. The list only falls; the census is
//! `live_tests::every_holder_of_the_vm_lends_the_world`.

pub(super) const STILL_BORROWS: &[(&str, usize)] =
    &[("capture/probe_mail.rs", 1), ("script_calls.rs", 1)];
