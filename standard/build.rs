// Tells its own builds from the framework's tests' build of its source (see src/lib.rs).
fn main() {
    println!("cargo::rustc-check-cfg=cfg(standard_crate)");
    println!("cargo::rustc-cfg=standard_crate");
}
