# Contributing to Dropwire

Thanks for your interest! Dropwire is free, open source, and privacy-first, and contributions that
keep it that way are very welcome.

## Principles (please respect these)

- **No accounts, no telemetry, no tracking.** Don't add sign-in, analytics, crash phone-home, or
  third-party SDKs. Privacy is the product.
- **Serverless by default.** The app must work with no infrastructure the user runs (DHT discovery
  + the public relay fallback). Self-hosting stays optional.
- **One-to-one transfers.** Keep the scope focused on direct device-to-device sending.
- **The `irohcore` boundary.** Only `core/` may depend on `iroh` / `iroh-blobs`. Everything else
  uses `irohcore`'s stable API. This is what keeps us safe from the pre-1.0 blobs churn.

## Getting set up

See [`docs/DEVELOPING.md`](docs/DEVELOPING.md) for toolchain and build/run instructions.

## Before opening a PR

```sh
cargo fmt --all
cargo clippy -p irohcore --all-targets --features test-utils -- -D warnings
cargo clippy -p dropwire --all-targets -- -D warnings
cargo test   -p irohcore --features test-utils
```

These are the checks CI runs. Keep `--features test-utils`: without it the nearby consent and
relay suites are compiled out and the test run still reports success.

- Keep changes focused; match the surrounding style.
- If you touch the transfer/resume path, add or update a test in `core/tests/`.
- If you change a flagged `VERIFY` spot (see `ARCHITECTURE.md` §13), note what you confirmed.

## Licensing

Dropwire is dual-licensed **MIT OR Apache-2.0**. By contributing, you agree your contributions are
licensed under the same terms.

## Reporting bugs / security issues

- Regular bugs: open an issue with steps to reproduce.
- Security vulnerabilities: **do not** open a public issue. Report privately via
  [GitHub private vulnerability reporting](https://github.com/muhamadjawdatsalemalakoum/dropwire/security/advisories/new).
  [`SECURITY.md`](SECURITY.md) covers what to do if that form is not available to you.
