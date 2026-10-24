# Code tiers copié

## whisper-rs 0.16.0 (Unlicense)

Copie de https://crates.io/crates/whisper-rs/0.16.0, sans `examples/` ni `.github/`.
Seul changement, marqué `Omega patch` :

- `src/whisper_grammar.rs`, `src/common_logging.rs` : `repr(i32)` sur toutes les cibles Windows. bindgen (clang) type les
  enums C en `int` sous MinGW comme sous MSVC ; l'original attendait `u32` sous MinGW et ne compilait pas.

À retirer dès qu'une version publiée compile sous `x86_64-pc-windows-gnu`.
