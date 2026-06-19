# Developer Folder Scanner and Policy Foundation

Historical terminology note: this architecture slice uses `project` for detected developer folders.
New product language should say shared folder or detected folder. Devbox is not built around
projects; it analyzes whatever folder the user shares. Loom is the codename for the deeper
source-control primitive underneath Devbox.

This slice adds a read-only scanner that classifies local project directories and explains default generated-artifact exclusions. It does not create snapshots, hash content, write metadata, sync files, or call language package managers.

## Current Detection

The scanner walks a local directory tree and records projects when it finds these marker files:

| Project kind | Signals |
| --- | --- |
| Node | `package.json`, with `package-lock.json`, `pnpm-lock.yaml`, or `yarn.lock` used to choose a rehydration hint |
| Rust | `Cargo.toml` |
| Python | `pyproject.toml`, `requirements.txt`, or `setup.py` |

The CLI exposes this as:

```text
devbox scan <path>
```

The command prints the canonical scan root, detected projects, project signals, rehydration hints, and policy exclusions.

## Default Generated-Artifact Policy

The scanner excludes generated or tool-owned directories before descending into them. These names are directory-boundary policy, not a rule that excludes ordinary regular files with the same names. Current defaults include:

- `.git`
- `node_modules`
- `.next`
- `dist`
- `build`
- `target`
- `.venv`
- `venv`
- `__pycache__`
- `.pytest_cache`
- `.turbo`
- `.gradle`
- `.cache`
- `coverage`

Each exclusion keeps an explanation string so later UI and `explain` surfaces can show why Devbox skipped a path.

## Deferred

The scanner intentionally does not implement:

- `.gitignore` parsing
- user, project, or team policy overrides
- full secret policy overrides or secret manager integration; local high-confidence block detection
  now runs in the snapshot builder
- file hashing or content-addressed object writes, except through the later snapshot builder
- SQLite-backed snapshot manifest persistence
- SQLite storage
- filesystem watching
- Git state capture beyond excluding `.git`
- networking or remote object storage
- package-manager execution or automated rehydration

Those pieces should land in later PR-sized slices after this read-only classification and policy vocabulary has stabilized.
