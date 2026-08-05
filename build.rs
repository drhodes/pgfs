// build.rs — capture the rustc toolchain version at compile time so the
// panic report (spec/app.py ReportsCrashes, spec/observe.py CrashReports)
// can include "built with <rustc>". The value is exposed via
// `option_env!("PGFS_RUSTC_VERSION")` in main.rs and degrades gracefully
// to "unknown" when the build script has not run (e.g. the Bazel build,
// which globs src/** only and does not run build scripts).
fn main() {
    let version = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    println!("cargo:rustc-env=PGFS_RUSTC_VERSION={version}");
    println!("cargo:rerun-if-changed=build.rs");
}
