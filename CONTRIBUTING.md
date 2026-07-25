# Contributing

Start with `DESIGN.md` and `AGENTS.md`. Changes should strengthen exploration,
geographical coherence, deterministic generation, or the thin infrastructure
needed to support them.

Before opening a pull request, run:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
```

Pull requests that intentionally alter generated worlds must state whether they
require a generation-version change and include updated golden tests.

