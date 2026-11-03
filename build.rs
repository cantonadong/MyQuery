fn main() {
    slint_build::compile("ui/main.slint").expect("compile Slint UI");
    let object = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("app-icon.o");
    let result = std::process::Command::new("x86_64-w64-mingw32-windres")
        .args(["-i", "app.rc", "-O", "coff", "-o"])
        .arg(&object)
        .status()
        .expect("run Windows resource compiler");
    assert!(result.success(), "compile application icon");
    println!("cargo:rustc-link-arg={}", object.display());
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=ui/icons/app.ico");
}
