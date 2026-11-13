# Building

- While testing in dev, build with `cargo build --release --features dev`, unless told otherwise. This enables dev-only menu options such as "Use fake data (dev only)".
- For normal releases, use plain `cargo build --release` (no `dev` feature).
