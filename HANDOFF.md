# plumbline — handoff

2026-09-13. plumbline is a standalone tool (binary `plumb`) that keeps a Rust
crate's shipped docs in lockstep with its real binary output and gates `cargo
publish` on it. It relates to rf like beads_rust does: an installed CLI invoked
by name, not vendored, not an rf feature.

## Where things stand

The self-hosting loop is closed. plumbline now guards its own publish on its own
contract, on the same terms it enforces on any other crate. All three
"Building this" steps in CONTRACT.md are done:

1. `Diagnostic { code, message }` threaded through every `Err` path. Emitted as
   `[CODE] message` on stderr. 25 codes in 5 families (USAGE/CONFIG/ENV/CAPTURE/
   GATE). Family is the stable promise; leaf codes refine within a family.
2. `plumb capabilities` emits plumbline's own contract envelope as JSON on
   stdout. `error_codes` is generated from `diagnostic::codes::ALL`, so the
   published catalog is exactly the set of codes the binary can raise. Runs
   anywhere; `main` dispatches it before loading a config.
3. `plumbline.json` flipped from contract-less to a full self-contract: `capture`
   runs `plumb capabilities`, `fixture` is a committed snapshot at
   `tests/fixtures/contract/capabilities.pl.json` (NOT packaged), `normalize_meta`
   drops the volatile `request_id` and `elapsed_ms`, and six self-claims are
   registered (contract_version, verb_set, exit_code_set, error_code_set, and
   the STRAY_BLOCK exit/family value claims).

## Verified this session (commit c1c28da, CI 34743052598 green)

- `plumb check` — all 6 claims match the committed fixture.
- `plumb capture --check` — idempotent. Comparison normalizes both sides (drops
  request_id + elapsed_ms) before comparing, so the stale meta snapshot in the
  fixture does not churn.
- `plumb preflight` — all 4 gates PASS on the clean committed tree, including
  gate 2 `fixture-fresh`, which was inactive while plumbline was contract-less.
  That gate is what makes plumbline self-guarding.
- `cargo fmt --check` clean, `cargo clippy --all-targets --locked -D warnings`
  clean, 31 tests pass (22 unit + 3 capabilities + 6 drift).

## Repo facts

- Crate v0.0.1, binary `plumb`, single dependency `serde_json = "1"`,
  Apache-2.0, repo rgfind/plumbline, at `~/p/plumbline`.
- `Cargo.toml` `include = ["src/**/*.rs", "README.md", "LICENSE"]`. So `tests/`,
  `CONTRACT.md`, and the fixture do NOT ship and do NOT trip the
  packaged-allowlist gate.
- CI: fmt --check, clippy -D warnings, test --locked. checkout@v5,
  dtolnay/rust-toolchain@stable.

## The gates (dynamic; only applicable ones run)

1. worktree-clean — always.
2. fixture-fresh — only when capture + fixture declared (now active for plumb).
3. docs-stray-block — always.
4. packaged-allowlist — always.
5. generated-fresh — only when generated blocks declared (plumb declares none).

## rf: the consumer, and where the external use stands

Corrected 2026-09-13 after inspecting rf. Earlier drafts of this handoff said rf
had never published and plumbline had never guarded a real publish. The first is
flat wrong; the second is only partly true.

- **rf is a live published crate.** On crates.io as `rf` (repo rgfind/rf),
  currently 0.0.5, 5 versions, 75 downloads. rf has shipped five times.
- **rf already builds its publish flow on plumbline.** `rf/RELEASING.md` makes
  `plumb preflight` exit 0 the stop-sign before `cargo publish` (5 gates).
  `rf/.github/workflows/contract-guard.yml` installs plumbline from git each run
  and runs `plumb check` + `plumb capture --check` on every push/PR.
  `rf/plumbline.json` is a mature 13-claim self-contract with a generated README
  block. All 5 preflight gates pass on rf's current 0.0.5 tree.
- **The guard evolved.** rf first guarded with an in-repo `xtask preflight`, then
  switched to external plumbline (rf commit 74abc1b, "Replace in-repo xtask
  guardrail with external plumbline tool"). That switch is recent, so rf's next
  release (0.0.6) is likely the first publish guarded by plumbline-as-external-
  tool rather than the old xtask. THAT is the accurate acid test — not
  "first-ever external use."

## What is NOT done: Phase 4 — publish plumbline itself to crates.io

Phase 4 is the ultimate acid test, not a dogfooding reward. plumbline can not be
a truly rigorous tool until it is used on itself in the process of being
published to crates.io. The machinery now exists and passes locally (the
self-contract). Still not ready: plumbline should first guard a real rf release
as the external tool, and likely wants a crates.io-cold README and a version
verb. crates.io is write-once, so the bar is high. Docs-in-lockstep rule stands.

## Two gaps found while inspecting rf (2026-09-13) — both now closed

1. **Stale local `plumb` — FIXED.** `~/.cargo/bin/plumb` was from commit
   16a51c79 (the original standup, pre-`capabilities`). Refreshed to 5ef66f5
   with `cargo install --git https://github.com/rgfind/plumbline.git plumbline
   --locked --force`. Verified: usage now lists `capabilities`, `plumb
   capabilities` emits all 25 codes, and `plumb preflight` on rf still exits 0
   with the fresh binary. What a releaser runs by hand now matches rf CI.
2. **rf CI ran only the light checks — FIXED.** Added
   `rf/.github/workflows/release-preflight.yml` (rf commit cd6a837): full
   `plumb preflight` (all five gates) fires on any `v*` tag push, backstopping
   the manual RELEASING.md stop-sign. It does not publish; `cargo publish` stays
   manual. Caveat: it fires on the tag, so it catches a bad tag but nothing
   blocks a publish that skips tagging — the human `plumb preflight` per
   RELEASING.md is still the real enforcement.

## Next actions, ranked

1. **Guard rf's next release (0.0.6) with external plumbline, for real.** That
   is the confirmed external use plumbline needs before its own Phase 4. Nothing
   to do until there is a release to cut.
2. Give plumb a crates.io-cold README and a `--version`, then Phase 4 (publish
   plumb itself) — only after 1.

## Watch-outs for the next session

- cwd resets to `~/p/rf/rf` between shell calls in this environment. `cd
  ~/p/plumbline` in every Bash command.
- `--notes`-style overwrite does not apply here, but note: `plumb capture`
  rewrites the fixture in place; run it only when you intend to reconcile.
- The fixture keeps a stale `request_id`/`elapsed_ms` on purpose — they are
  normalized away, never compared. Do not "fix" them.
- Adding a new error code is free (no contract_version bump) as long as it stays
  inside an existing family. Changing a code's family or exit is breaking.
