fn main() {
    if cfg!(target_os = "windows") {
        println!("cargo:rustc-link-arg=/STACK:16777216");
        println!("cargo:rustc-link-arg-bin=remote-code=/STACK:16777216");
    }
}
