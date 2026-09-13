# plumbline contract

This is the reference for `plumb capabilities --json`: the machine-readable
description of plumbline's own command surface, exit codes, and diagnostics.

**Status: implemented.** All three steps of "Building this" are done. Every
`Err` path carries a `Diagnostic` with a catalog code, printed as `[CODE]
message` on stderr; `plumb capabilities` emits the envelope below as live JSON,
with `error_codes` built straight from the same code catalog; and
`plumbline.json` now declares a full self-contract, so `plumb preflight` gates
plumbline's own publish on the self-claims below. A code in this catalog is a
promise, and the promise is real only when the running tool emits the code. See
"Building this" at the end.

`contract_version` is `1`.

## Why a contract

plumbline gates other crates' publishes. crates.io is write-once, so a
published contract must hold. plumbline must meet the same bar it enforces:
describe its own surface as data, capture it to a fixture, and gate its own
publish on it. This is the acid test — plumbline guarding itself.

## Envelope

The envelope mirrors the shape plumbline already checks for its targets, so the
same claim machinery (modes `value` / `keys` / `set`, `normalize_meta`) applies
unchanged. The wrapper carries meta; the contract lives in `data[0]`.

```json
{
  "ok": true,
  "tool_version": "0.0.1",
  "meta": { "request_id": "...", "elapsed_ms": 0 },
  "commands": ["check", "capture", "preflight", "capabilities"],
  "warnings": [],
  "errors": [],
  "data": [
    {
      "contract_version": "1",
      "tool_version": "0.0.1",
      "exit_codes": { },
      "global_flags": [ ],
      "verbs": { },
      "gates": [ ],
      "value_domains": { },
      "error_codes": { },
      "warning_codes": []
    }
  ]
}
```

`meta` fields are volatile. They are listed in `normalize_meta` and dropped
before any two captures are compared.

## Exit codes

| code | meaning | retryable |
|---|---|---|
| `0` | success; every gate and claim held | no |
| `1` | a gate, claim, capture, or config step failed | no |
| `2` | usage error (unknown verb, or `--config` without a path) | no |

No diagnostic is retryable. plumb is a deterministic local guard. A re-run
against an unchanged tree returns the same verdict.

## Verbs

| verb | flags | needs a contract | summary |
|---|---|---|---|
| `check` | none | no | claims equal the fixture; no stray blocks |
| `capture` | `--check` | yes | rewrite the fixture from a fresh run; re-render blocks |
| `preflight` | none | no | the publish stop-sign; run every applicable gate |
| `capabilities` | none | no | emit this contract as JSON |

"Needs a contract" means the verb requires a declared `capture` and `fixture`. A
contract-less crate runs `check`, `preflight`, and `capabilities`; `capture`
refuses cleanly with `NO_CONTRACT`.

## Gates

`preflight` runs each gate that applies to the crate.

| gate id | applies when |
|---|---|
| `worktree-clean` | always |
| `fixture-fresh` | a `capture` and `fixture` are declared |
| `docs-stray-block` | always |
| `packaged-allowlist` | always |
| `generated-fresh` | generated blocks are declared |

## Diagnostic families

A **family** groups codes by the nature of the fault a consumer must react to.
The family is the stable promise. A leaf code is a refinement inside its family.

| family | the consumer's question | exit |
|---|---|---|
| `USAGE` | did I invoke plumb wrong? | 2 |
| `CONFIG` | is my config file wrong? | 1 |
| `ENV` | did the host, filesystem, or tooling fail a precondition? | 1 |
| `CAPTURE` | did the crate's own build or binary fail to produce what was asked? | 1 |
| `GATE` | is the crate out of lockstep, so it must not publish? | 1 |

A consumer that wants coarse behavior matches the `family`. A consumer that
wants detail matches the leaf `code`. Match the family for stability (see
"Stability policy").

## Error codes

Each code carries a `family`, a `meaning`, its `exit`, and the verbs that emit
it. The `meaning` is prose and may be reworded. The `code`, its `family`, and
its `exit` are the promise.

| code | family | exit | meaning |
|---|---|---|---|
| `USAGE` | USAGE | 2 | unknown verb, or `--config` given without a path value |
| `CONFIG_UNREADABLE` | CONFIG | 1 | the config file could not be read |
| `CONFIG_INVALID_JSON` | CONFIG | 1 | the config file is not valid JSON |
| `CONFIG_SCHEMA` | CONFIG | 1 | a field is missing or mistyped inside a declared block |
| `CONFIG_INCOHERENT` | CONFIG | 1 | claims declared without a fixture, or generated blocks without a capture |
| `ALLOWLIST_UNKNOWN` | CONFIG | 1 | the `package_allowlist` value is not recognized |
| `WORKDIR_UNREADABLE` | ENV | 1 | the working directory could not be determined |
| `FIXTURE_UNREADABLE` | ENV | 1 | the committed fixture could not be read or parsed |
| `SURFACE_UNREADABLE` | ENV | 1 | a doc surface could not be read |
| `WRITE_FAILED` | ENV | 1 | the fixture or a doc surface could not be written |
| `GIT_UNAVAILABLE` | ENV | 1 | `git status` could not run |
| `PACKAGE_LIST_FAILED` | ENV | 1 | `cargo package --list` failed (for example a dirty tree without `--allow-dirty`) |
| `NO_CONTRACT` | CAPTURE | 1 | `capture` was invoked on a crate that declares no capture or fixture |
| `BUILD_FAILED` | CAPTURE | 1 | the build command exited non-zero |
| `CAPTURE_RUN_FAILED` | CAPTURE | 1 | the capture command was empty or exited non-zero |
| `CAPTURE_NOT_JSON` | CAPTURE | 1 | captured stdout was not valid JSON |
| `RENDER_FAILED` | CAPTURE | 1 | a render command was empty, exited non-zero, or emitted non-UTF-8 |
| `MARKER_MISSING` | CAPTURE | 1 | a block's BEGIN or END marker was not found on its surface |
| `WORKTREE_DIRTY` | GATE | 1 | uncommitted paths in the tree to be packaged |
| `FIXTURE_STALE` | GATE | 1 | a fresh capture differs from the committed fixture |
| `CLAIM_DRIFT` | GATE | 1 | a registered claim no longer equals its fixture field |
| `STRAY_BLOCK` | GATE | 1 | a surface carries a GENERATED block with no declared renderer |
| `PACKAGED_LEAK` | GATE | 1 | a packaged file falls outside the include allowlist |
| `BLOCK_STALE` | GATE | 1 | a generated block differs from a fresh render |
| `PREFLIGHT_FAILED` | GATE | 1 | one or more gates failed (the aggregate; the gate codes above are the ground truth) |

## Warning codes

`warning_codes` is empty. Every plumb diagnostic today is fatal. The
informational lines (`already current`, `regenerated X`) are progress notes, not
warnings. The list grows only when plumb gains a non-fatal condition.

## Self-claims

Once `capabilities --json` exists, plumbline pins these facts about itself,
exactly as it pins facts about a target crate.

| claim id | mode | fixture_path |
|---|---|---|
| `contract_version` | value | `data.0.contract_version` |
| `verb_set` | keys | `data.0.verbs` |
| `exit_code_set` | keys | `data.0.exit_codes` |
| `error_code_set` | keys | `data.0.error_codes` |
| `stray_block_exit` | value | `data.0.error_codes.STRAY_BLOCK.exit` |
| `stray_block_family` | value | `data.0.error_codes.STRAY_BLOCK.family` |

`error_code_set` pins the full set of codes. A per-code `value` claim pins that
code's family and exit, so a silent re-family or re-exit is caught.

## Stability policy

The promise is the `code` identifier, its `family`, and its `exit`. Change is
governed by whether it can break a consumer.

**Free at any version** (non-breaking):

- Reword any `meaning`.
- Add a new code. A consumer sees an unknown code and still reads the exit and
  the family prefix.
- Add a new family.
- Split a coarse code into finer leaf codes **within the same family**. A
  family-matcher does not break; only a consumer matching the retired leaf does,
  which is why detail-matchers are told the leaf set can grow.

**Needs a `contract_version` bump** (breaking):

- Rename or remove a family.
- Change a code's `family` or `exit`.
- Rename or remove a leaf code that a consumer may match directly.

Families are few and map to the obvious phases, so they are low-risk to fix
early. Leaf granularity is a cheap refinement, safe to grow inside a stable
family. Pre-publish, the whole catalog is free to change: the only consumer is
plumbline's own self-claim, and `plumb capture` re-reconciles it.

## Building this

The catalog is complete so the implementation is mechanical:

1. **Done.** A `Diagnostic { code, message }` type, where `code` is a `Code {
   name, family }` from the catalog, carried through every `Err` path. The
   running tool emits it as `[STRAY_BLOCK] ...` on stderr. Threading it surfaced
   two codes the read/gate enumeration had missed — `SURFACE_UNREADABLE` and
   `WRITE_FAILED` (both ENV) — added without a `contract_version` bump, as the
   stability policy allows.
2. **Done.** The `capabilities` verb emits the envelope above on stdout. It
   needs no config (it describes the tool, not a project), so it runs anywhere;
   `main` dispatches it before loading a config. `error_codes` is generated from
   `diagnostic::codes::ALL`, so the published catalog is exactly the set of codes
   the binary can raise — the doc cannot claim a code the tool lacks, nor omit
   one it has. `meta` carries a volatile `request_id` and `elapsed_ms`; both are
   normalized away before any two captures are compared.
3. **Done.** `plumbline.json` flipped from contract-less back to a full
   self-contract: `capture` runs `plumb capabilities`, `fixture` is a committed
   snapshot at `tests/fixtures/contract/capabilities.pl.json` (not packaged),
   `normalize_meta` drops the volatile `request_id` and `elapsed_ms`, and the
   six self-claims above are registered. `plumb preflight` now runs the
   `fixture-fresh` gate against a live capture, so plumbline guards itself on the
   same terms it enforces on any other crate.
