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

## What is NOT done: Phase 4 — publish to crates.io

Phase 4 is the ultimate acid test, not a dogfooding reward. plumbline can not be
a truly rigorous tool until it is used on itself in the process of being
published to crates.io. The machinery that makes the publish "the acid test" now
exists and passes locally. But plumbline is NOT ready for that step: it needs
real external use first (it has one consumer, rf, and has never guarded a real
publish). crates.io is write-once, so the bar is high.

Docs-in-lockstep rule stands: docs must progress in lockstep with code at the
moment of publishing to crates.io.

## Next actions, ranked

1. **Let plumbline earn its publish through use.** Wire rf's real publish flow
   to call `plumb preflight` as the pre-publish gate and run it on rf's next
   actual release. That is the external use plumbline needs before its own
   Phase 4. (rf's contract fixture lives at
   `rf/tests/fixtures/contract/capabilities.rc.json`.)
2. Consider whether plumb needs a `--version` and a `README` that a crates.io
   visitor reads cold, before any publish.
3. Phase 4 itself (publish plumb to crates.io) — only after 1 has happened at
   least once for real.

## Watch-outs for the next session

- cwd resets to `~/p/rf/rf` between shell calls in this environment. `cd
  ~/p/plumbline` in every Bash command.
- `--notes`-style overwrite does not apply here, but note: `plumb capture`
  rewrites the fixture in place; run it only when you intend to reconcile.
- The fixture keeps a stale `request_id`/`elapsed_ms` on purpose — they are
  normalized away, never compared. Do not "fix" them.
- Adding a new error code is free (no contract_version bump) as long as it stays
  inside an existing family. Changing a code's family or exit is breaking.
