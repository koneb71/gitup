//! Embeds the icon and version metadata into the Windows executable.
//!
//! Without this a Windows build is a blank-icon `gitup.exe` with no publisher
//! or version in its Properties, which reads as untrustworthy — and Windows
//! has no equivalent of the `.app` bundle to carry that information alongside
//! the binary. Everything here is cosmetic, so a failure warns rather than
//! breaking the build.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/gitup.ico");
    println!("cargo:rerun-if-changed=build.rs");

    // The *target*, not the host: this has to run when cross-compiling to
    // Windows from a Linux CI runner, where `cfg!(windows)` would be false.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let version = env!("CARGO_PKG_VERSION");
    let mut resource = winresource::WindowsResource::new();
    resource
        .set_icon("assets/icon/gitup.ico")
        .set("ProductName", "Gitup")
        .set("FileDescription", "A modern graphical Git client")
        .set("ProductVersion", version)
        .set("FileVersion", version)
        .set("LegalCopyright", "MIT licensed. See LICENSE.");

    if let Err(error) = resource.compile() {
        println!("cargo:warning=icon not embedded: {error}");
    }
}
