# plumbline

Keep a Rust crate's shipped docs in lock-step with its real binary output, and
gate `cargo publish` on it.

crates.io is write-once. Once a version is published, its README and metadata can
never change for that version. So the docs and the code must agree at the exact
moment of publishing. plumbline is the plumb-line: it proves the docs hang true
before the archive goes up.

The crate is `plumbline`; the command is `plumb` (as `beads_rust` ships `br`).

## What it checks

- **The fixture matches the binary.** It builds the crate, runs a capture command
  (e.g. a `--json` capabilities dump), and confirms the committed contract fixture
  still equals that fresh output.
- **The docs match the fixture.** Every registered contract fact still equals its
  fixture field. A drifted fact fails the check.
- **Generated doc blocks match the binary.** A block marked as generated is
  re-rendered from a real run of the binary in a throwaway sample tree, and must
  be byte-identical to what the doc ships. A hand-edit that drifts fails.
- **Packaged files stay within the allowlist.** Every file `cargo package` would
  ship matches an `include` glob from `Cargo.toml` (or is cargo's own metadata),
  so no build tool, CI file, or test fixture leaks into the archive.
- **The worktree is clean.** `cargo publish` packages the working tree, so an
  untidy tree could ship un-reviewed files.

## The niche

Release tools (`cargo-release`, `release-plz`) automate the ceremony. Semver tools
(`cargo-semver-checks`, `cargo-public-api`) check the type surface. Provenance
tools check bytes against a commit. None check that the prose and examples a
reader sees still describe what the binary does. plumbline fills that gap.

## Commands

```sh
plumb check                # claims still match the fixture; no stray blocks
plumb capture              # rewrite the fixture and re-render doc blocks
plumb capture --check      # assert the fixture is not stale (CI gate)
plumb preflight            # the publish stop-sign: every gate, or non-zero exit
```

The one rule: never run `cargo publish` unless `plumb preflight` exits 0.

## Config

A project describes itself in one JSON file (default `plumbline.json` in the
working directory, or `--config <path>`). The working directory is the crate
root; every relative path in the config resolves there.

```jsonc
{
  "capture": {
    "build":   ["cargo", "build", "--quiet", "--bin", "rf"],
    "command": ["rf", "capabilities", "--json"],
    "env":     { "SOURCE_DATE_EPOCH": "0" }
  },
  "fixture": "tests/fixtures/contract/capabilities.rc.json",
  "normalize_meta": ["request_id", "ts_iso", "data_hash", "elapsed_ms"],
  "claims": [
    { "id": "tool_version", "fixture_path": "tool_version",
      "mode": "value", "expected": "0.0.5" }
  ],
  "surfaces": ["README.md", "CHANGELOG.md"],
  "generated": [
    { "id": "readme-example", "surface": "README.md",
      "render": ["rf", "content", "timeout", ".", "--human"],
      "env": { "SOURCE_DATE_EPOCH": "0", "NO_COLOR": "1" },
      "prompt": "$ rf content timeout .",
      "tree": { "git_init": true,
                "files": { "config.py": "timeout = 30\n" } } }
  ],
  "package_allowlist": "cargo-include"
}
```

### Claims

A claim pins one contract fact to a fixture field:

- `fixture_path` — a dotted path into the fixture (`data.0.exit_codes`; a numeric
  segment indexes an array; an empty path is the root).
- `mode` — `value` (deep equality), `keys` (an object's keys, order-independent),
  or `set` (an array of strings, order-independent).
- `expected` — the pinned value.

### Generated blocks

A generated block is delimited by HTML comments, invisible in rendered Markdown:

```
<!-- BEGIN GENERATED:readme-example -->
...body plumbline owns...
<!-- END GENERATED:readme-example -->
```

plumbline owns the body and rewrites it from a real run of the binary against the
block's `tree` (a throwaway directory built fresh for the render, then discarded).
The marker lines are preserved.

## License

Apache-2.0.
