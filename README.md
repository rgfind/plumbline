# plumbline

Use `plumb` to check that the documentation in a Rust crate describes the
binary that `cargo publish` will package. Run `plumb preflight` before you
publish a crate to crates.io.

`plumb` checks a project contract. It does not publish a crate.

## Install

```sh
cargo install plumbline
```

Check the installed version from any directory:

```sh
plumb --version
```

## Before you publish

Run these commands from the crate root:

```sh
plumb preflight
cargo publish
```

`plumb preflight` checks the fixture, claims, generated documentation, package
file list, and worktree. It exits with a nonzero status if a check fails.

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
  "package_allowlist": "cargo-include"
}
```

`capture` builds the crate and records the JSON output from its command in the
fixture. `normalize_meta` lists output fields that change for each run. Do not
put stable release facts in that list.

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

## License

Apache-2.0.
