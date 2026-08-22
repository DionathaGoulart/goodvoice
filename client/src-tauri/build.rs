fn main() {
    delay_load_comctl32();
    tauri_build::build();
}

/// Keeps `cargo test` able to start on Windows.
///
/// The tray menu mutates its items, which pulls `SetWindowSubclass`,
/// `RemoveWindowSubclass`, `DefSubclassProc` and `TaskDialogIndirect` into the
/// library. Those are exported by comctl32 **version 6**, which a process gets
/// only by asking for it in a manifest. `tauri_build` embeds a manifest that
/// does ask — into the binaries, through `rustc-link-arg-bins`. The library's
/// own unit-test executable is not a binary target, so it gets no manifest,
/// binds to the version 5 in `system32`, and dies before `main` with
/// `STATUS_ENTRYPOINT_NOT_FOUND`: a whole suite of passing tests reported as
/// one unexplained `0xc0000139`.
///
/// Handing that manifest to every target instead is what one tries first, and
/// it fails — the binaries already have one, and two of them is `CVT1100:
/// duplicate resource`. Cargo has no "link arg for the library's test" to aim
/// at the one target that is short.
///
/// So comctl32 is delay-loaded rather than bound at startup. The test binary
/// never calls into it and now never loads it; the app resolves it on the first
/// call, by which time its manifest has long since asked for version 6.
/// See DR-17.
fn delay_load_comctl32() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }

    println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
    // The thunk that does the loading lives here; without it the delayed
    // imports are unresolved symbols.
    println!("cargo:rustc-link-lib=delayimp");
}
