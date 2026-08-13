//! Embed the application icon into the Windows executable.
//!
//! Without this the exe carries the default Rust binary icon in Explorer, the
//! taskbar pin and the shortcut, however good the *window* icon is — they come
//! from the file, not from the running process.
//!
//! **It never fails the build.** Embedding needs a resource compiler (`rc.exe`
//! from the Windows SDK, or `windres`), and a machine without one can still
//! build and run BuzzAnimate perfectly well. A missing icon is a cosmetic
//! problem; a build that refuses to produce a working editor is not.

fn main() {
    #[cfg(windows)]
    {
        let icon = "../../assets/buzzanimate.ico";
        println!("cargo:rerun-if-changed={icon}");

        if !std::path::Path::new(icon).exists() {
            println!("cargo:warning=no {icon}; the executable keeps the default icon");
            return;
        }

        let mut resource = winresource::WindowsResource::new();
        resource.set_icon(icon);
        resource.set("ProductName", "BuzzAnimate");
        resource.set("FileDescription", "BuzzAnimate");
        resource.set("CompanyName", "Buzzcaf Media");
        if let Err(e) = resource.compile() {
            println!("cargo:warning=could not embed the icon ({e}); using the default");
        }
    }
}
