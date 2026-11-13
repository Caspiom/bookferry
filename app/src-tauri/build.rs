fn main() {
    println!("cargo:rustc-check-cfg=cfg(bookferry_e2e)");
    println!("cargo:rerun-if-env-changed=BOOKFERRY_E2E");
    if std::env::var("BOOKFERRY_E2E").as_deref() == Ok("1") {
        println!("cargo:rustc-cfg=bookferry_e2e");
    }
    tauri_build::build()
}
