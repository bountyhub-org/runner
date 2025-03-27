fn main() {
    let version = std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION to be set");
    println!("cargo:rustc-env=BUILD_VERSION={version}");
}
