//! Gives the Windows executable the icon. Only when building on Windows: that's where the resource compiler is, and
//! other platforms have no use for it (the macOS app's icon is added when it's packaged).

fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    winresource::WindowsResource::new()
        .set_icon("../../assets/icon.ico")
        .set("ProductName", "ephys-rs")
        .set("FileDescription", "ephys-rs")
        .compile()
        .expect("embed the icon");
}
