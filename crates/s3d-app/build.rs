//! Windows: put the program icon and version details into simply3d.exe, so Explorer, the
//! taskbar and shortcuts show them.

fn main() {
    println!("cargo:rerun-if-changed=icons/simply3d.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("icons/simply3d.ico")
            .set("ProductName", "Simply3D")
            .set("FileDescription", "Simply3D")
            .set("LegalCopyright", "Simply3D contributors");
        if let Err(e) = res.compile() {
            // a missing resource compiler only costs the icon, not the build
            println!("cargo:warning=no program icon: {e}");
        }
    }
}
