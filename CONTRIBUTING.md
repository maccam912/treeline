# Contributing

Start with [`DESIGN.md`](DESIGN.md) and [`AGENTS.md`](AGENTS.md). Changes should
strengthen exploration, the coherence of the measured world, deterministic
generation, or the thin infrastructure those need.

Before opening a pull request:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Browser-only code is behind `cfg(target_arch = "wasm32")` and needs its own
pass:

```sh
cargo clippy -p client --target wasm32-unknown-unknown --all-targets -- -D warnings
```

A change that alters generated worlds must say whether it needs a generation
version change, and include updated golden tests.

A change to an embedded bundle artifact — its bytes, coordinate frame, sampler,
or the meaning of a layer — needs a new settings identity. Never replace bundle
bytes while keeping an identity a saved world could be using.
