# crates/tokenx Guidelines

## Localization (rust-i18n)

User-facing strings in this crate are localized with `rust-i18n` (v3). The
engine crate (`tokenx-engine`) stays English.

- Locale files live in `crates/tokenx/locales/`, one file per locale for each
  stable product surface: `<scope>.<locale>.yml` (e.g.
  `subscription.en.yml`, `subscription.zh-CN.yml`). Small related namespaces
  may share one surface catalog only when its allowed prefixes are declared
  explicitly in `build.rs`; do not create mixed catalogs that span unrelated
  UI surfaces. rust-i18n derives the locale from the file stem's last dot
  segment, so the locale MUST be the suffix; names like `en.cli.yml` silently
  register under a bogus locale.
- Files use rust-i18n v1 format: nested keys WITHOUT a top-level locale
  wrapper. Nested maps flatten to dot keys, so `cli:` → `error:` → `x` in
  `cli.en.yml` becomes the key `cli.error.x`.
- Key naming: lowercase dotted hierarchy with a module prefix to avoid
  cross-module collisions, e.g. `cli.error.xxx`, `tui.footer.xxx`,
  `tui.ui.stats.xxx`. The scope in the filename and the key prefix should
  match the owning module.
- `en` is the source language and the configured fallback; every key MUST
  exist in the `en` file. Interpolation uses `%{param}` with
  `rust_i18n::t!("key", param = value)`; `t!` with parameters returns
  `Cow<str>` (use `.into_owned()` where a `String` is needed).
- Call sites use `rust_i18n::t!("key")`. The `i18n!` macro is invoked once at
  the crate root in `main.rs` (with `fallback = "en"`); `build.rs` tracks the
  locales directory so translation edits trigger a rebuild.
- `build.rs` rejects incomplete catalogs before Rust compilation. Every scope
  must provide the same locale set, every non-English catalog must match the
  English leaf-key tree and interpolation names, and keys must remain under
  the owning product-surface prefix.
- Language selection priority: `--language` flag > `language` in
  settings.json > environment (`LC_ALL`, then `LANG`; any `zh*` value maps to
  `zh-CN`) > English. Both the flag and the settings key are typed
  (`crate::i18n::Language`); unknown values are hard errors, never a silent
  English fallback (see ADR 0001). Resolution lives in
  `crate::i18n::resolve_locale` / `init`, called early in `main.rs::run`.
- Tests: never mutate the global locale. Assert translations with the
  explicit-locale form `rust_i18n::t!("key", locale = "zh-CN")`, and pass
  `--language en` in subprocess tests that assert English output so they do
  not depend on the host locale.

To add translations for a product surface, create one `<scope>.<locale>.yml`
for every supported locale with the same nested key tree, then replace
literals with `t!()` calls.
