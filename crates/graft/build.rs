//! Embeds the app icon and version info into graft.exe on Windows.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon/graft.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon/graft.ico")
            .set("ProductName", "Graft")
            .set("FileDescription", "Graft — a fast git GUI");
        if let Err(err) = res.compile() {
            // A missing resource compiler must not break the build; the window
            // icon is set at runtime anyway.
            println!("cargo:warning=could not embed the exe icon: {err}");
        }
    }
}
