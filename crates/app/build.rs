//! Link settings for the game executable.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if os == "windows" && env == "msvc" {
        // Load Steam Audio's library at the first call into it instead of
        // before `main`, so an exported game can unpack the copy it carries
        // first (see `engine::standalone`).
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:phonon.dll");
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
    } else if os == "linux" {
        // Find `libphonon.so` next to the executable, where EXPORT GAME
        // puts it.
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,$ORIGIN");
    }
}
