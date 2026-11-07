//! Embeds the Drumpad icon and version metadata into the Windows executable.
//!
//! `winresource` shells out to the resource compiler (`rc.exe`) that ships with
//! the Windows 11 SDK. The build fails with a clear panic if the SDK is missing,
//! rather than silently shipping an executable without an icon.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/drumpad.ico");

    #[cfg(windows)]
    {
        let mut resource = winresource::WindowsResource::new();
        resource.set_icon("assets/drumpad.ico");
        resource.set("ProductName", "Drumpad");
        resource.set("FileDescription", "Drumpad drum sampler");
        resource.set("CompanyName", "Drumpad contributors");
        resource.set("LegalCopyright", "MIT OR Apache-2.0");
        resource.set("OriginalFilename", "Drumpad.exe");
        resource.set("ProductVersion", env!("CARGO_PKG_VERSION"));
        resource.set("FileVersion", env!("CARGO_PKG_VERSION"));

        if let Err(error) = resource.compile() {
            panic!("failed to compile the Windows resources: {error}");
        }
    }
}
