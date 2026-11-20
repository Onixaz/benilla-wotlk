//! Stamps the commit this binary was built from, and links the main thread's stack reserve.

/// `benilla_app::MAIN_THREAD_STACK`, which a test there holds this to.
const MAIN_THREAD_STACK: usize = 8_388_608;

fn main() {
    benilla_buildstamp::emit();
    // The VM runs on the main thread, whose stack is the image's reserve: Lua's own C-call limit
    // ends a runaway recursion before it does.
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.contains("windows-msvc") {
        println!("cargo:rustc-link-arg-bins=/STACK:{MAIN_THREAD_STACK}");
    } else if target.contains("windows-gnu") {
        println!("cargo:rustc-link-arg-bins=-Wl,--stack,{MAIN_THREAD_STACK}");
    }
}
