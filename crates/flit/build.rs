fn main() {
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        for dll in ["d3d11", "dcomp", "dwrite", "dwmapi", "imm32", "ole32", "oleaut32", "shell32", "combase"] {
            println!("cargo:rustc-link-arg-bins=/DELAYLOAD:{dll}.dll");
        }
        println!("cargo:rustc-link-arg-bins=delayimp.lib");
        println!("cargo:rustc-link-arg-bins=/IGNORE:4199");
    }
}
