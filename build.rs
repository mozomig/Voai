fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=native/TelegramHTTP.m");
        cc::Build::new()
            .file("native/TelegramHTTP.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("telegram_http");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Network");
    }
}
