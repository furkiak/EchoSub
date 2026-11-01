fn main() {
    link_clang_runtime_on_macos();
    tauri_build::build()
}

/// macOS: ONNX Runtime'ın CoreML kısmı `___isPlatformVersionAtLeast` sembolünü kullanır; Rust bu
/// sembolü sağlamaz, Apple clang'ın çalışma zamanı kütüphanesinde (libclang_rt.osx.a) bulunur.
fn link_clang_runtime_on_macos() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let dir = std::process::Command::new("xcrun")
        .args(["clang", "--print-resource-dir"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
    if let Some(dir) = dir {
        println!("cargo:rustc-link-search={dir}/lib/darwin");
        println!("cargo:rustc-link-lib=static=clang_rt.osx");
    } else {
        println!("cargo:warning=xcrun clang bulunamadı; CoreML bağlaması başarısız olabilir");
    }
}
