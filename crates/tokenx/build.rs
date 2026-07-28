fn main() {
    // The rust-i18n `i18n!` macro embeds translations at compile time, so the
    // crate must rebuild whenever any locale file changes.
    println!("cargo:rerun-if-changed=locales");
}
