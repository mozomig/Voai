fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=native/TelegramHTTP.m");
        cc::Build::new()
            .file("native/TelegramHTTP.m")
            .flag("-fobjc-arc")
            .flag("-fblocks")
            .compile("telegram_http");
        println!("cargo:rerun-if-changed=native/Updates.m");
        println!("cargo:rerun-if-env-changed=VOAI_SPARKLE_SDK");
        let mut updates = cc::Build::new();
        updates
            .file("native/Updates.m")
            .flag("-fobjc-arc")
            .flag("-fblocks");
        if let Some(sdk) = std::env::var_os("VOAI_SPARKLE_SDK") {
            updates.define("VOAI_WITH_SPARKLE", None);
            updates.flag(format!("-F{}", sdk.to_string_lossy()));
        }
        updates.compile("voai_updates");
        println!("cargo:rustc-link-lib=framework=Foundation");
        println!("cargo:rustc-link-lib=framework=Network");
    }
}
