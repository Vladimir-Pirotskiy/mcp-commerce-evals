# Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings   # includes clippy::pedantic
cargo test --workspace
cargo doc --no-deps --workspace                          # with RUSTDOCFLAGS="-D warnings"
cargo deny --offline check licenses bans sources          # no network; see deny.toml
cargo deny check advisories                              # needs network (RustSec); CI runs it
```

`Cargo.lock` is committed. Dependencies are taken after a 14-day cooldown: a version is used once
it has been published for two weeks, and Dependabot is set to the same delay (`cooldown` in
`.github/dependabot.yml`). The exception is a security fix, taken at once: `rustls-webpki` 0.103.16
(published 2026-10-20) and `webpki-root-certs` 1.0.11 (2026-10-23) are in the lock for
RUSTSEC-2026-0382 and RUSTSEC-2026-0383. The lock was first generated with nightly cargo
(the build uses 1.99.0):

```sh
cargo +nightly generate-lockfile -Zunstable-options --publish-time 2026-10-19T00:00:00Z
```

The tests need no network and no key. The integration test starts the store on a random port inside
the test process and runs the whole tool-level eval against it, with a floor on hit@5 and MRR (CI's eval job gates the same numbers with `--min-hit5 0.92 --min-mrr 0.91`) and
a requirement that the first five queries (exact part numbers) rank first. Another test checks
that every expected id exists in the demo catalog, and `expected_sets` rederives every expected set
from catalog attributes. The assistant loop, the retries, the exit codes and the report metadata
are tested against mocks and the in-process store.
