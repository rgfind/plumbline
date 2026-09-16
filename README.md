# plumbline

## Release verification

Run the declared local CI gates before a push:

```text
plumb verify --stage=ci --json
```

For a release candidate, push the commit, then run `plumb preflight --wait --json`
to poll only its configured GitHub Actions proof for up to 30 seconds. Use
`plumb preflight --json` when the proof must be checked once. Then run
`plumb release --dry-run --json`. `plumb release --yes` is the separate,
explicit tag mutation.

Use `plumb` to check that the documentation in a Rust crate describes the
binary that `cargo publish` will package. Run `plumb preflight` before you
publish a crate to crates.io.

`plumb` checks a project contract. Its local `release` command runs only a
publish dry run. CI does the real crate publication after the pushed tag passes
its checks.

## Install

```sh
cargo install plumbline
```

Check the installed version from any directory:

```sh
plumb --version
```

## Release a crate

First, update the package version and add its H2 entry to `CHANGELOG.md`.
Commit those files and push the configured release branch. Then run this from
the crate root:

```sh
plumb release
```

`plumb release` requires a clean worktree on its configured branch, a
synchronized upstream, a changelog entry for the package version, and no local
or remote version tag. It runs `plumb preflight` and
`cargo publish --dry-run --locked`, then creates an annotated `v<version>` tag
and atomically pushes the branch and tag. It does not run a real `cargo publish`.
The tag workflow in CI reruns preflight, publishes the crate, and then creates
the source release entry.

## Configure a crate

Create `plumbline.json` in the crate root. `plumb` resolves its paths from that
directory.

This example uses the `rf` command. Replace its command names, paths, and
expected values with values for your crate.

```json
{
  "capture": {
    "build": ["cargo", "build", "--quiet", "--bin", "rf"],
    "command": ["rf", "capabilities", "--json"],
    "env": {"SOURCE_DATE_EPOCH": "0"}
  },
  "fixture": "tests/fixtures/contract/capabilities.json",
  "normalize_meta": ["request_id", "elapsed_ms"],
  "surfaces": ["README.md"],
  "claims": [
    {
      "id": "tool_version",
      "fixture_path": "tool_version",
      "mode": "value",
      "expected": "0.0.6"
    }
  ],
  "generated": [
    {
      "id": "readme-example",
      "surface": "README.md",
      "render": ["rf", "content", "timeout", ".", "--human"],
      "prompt": "$ rf content timeout .",
      "tree": {"files": {"config.py": "timeout = 30\n"}}
    }
  ],
  "package_allowlist": "cargo-include",
  "release": {"branch": "main", "remote": "origin"}
}
```

`capture` builds the crate and records the JSON output from its command in the
fixture. `normalize_meta` lists output fields that change for each run. Do not
put stable release facts in that list.

The `release` object is optional for `check`, `capture`, and `preflight`. It is
required for `plumb release`; both `branch` and `remote` must be nonempty
strings.

## Check facts and examples

A claim compares an expected value with a field in the fixture. `fixture_path`
is a dotted path. Use a number to select an array item, for example
`data.0.exit_codes`.

`mode` selects the comparison:

- `value` checks complete JSON equality.
- `keys` checks the keys in a JSON object.
- `set` checks a JSON array of strings without order.

A generated block is documentation that `plumb` recreates from a command in a
temporary sample tree. Put the following markers in the surface file:

```markdown
<!-- BEGIN GENERATED:readme-example -->
old generated text
<!-- END GENERATED:readme-example -->
```

Run these commands while you update a release:

```sh
plumb capture
plumb check
plumb capture --check
```

`plumb capture` writes a new fixture and rewrites generated blocks. Review and
commit those files together. `plumb check` compares claims and scans for unknown
generated blocks. `plumb capture --check` verifies that the committed fixture
still matches fresh command output without writing files.

## Package files

Set `package_allowlist` to `cargo-include` to make `plumb` check the Cargo
`include` list. This stops test files and release tooling from entering the
published crate by mistake.

`CHANGELOG.md` is release control material. Keep it outside Cargo's `include`
list; it does not enter the crate package.

## License

Apache-2.0.
