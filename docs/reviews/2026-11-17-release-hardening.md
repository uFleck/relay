# 2026-11-17 — Release hardening: version fields, prereleases, budget gate

Status: complete; Opus review pending.

This record covers the fix pass for the release audit findings 27, 29, 30 and
the installer fallback (14) and budget placement (16) in
[2026-11-17-scaffold.md](2026-11-17-scaffold.md#release-audit--2026-11-17). It
does not redo the scaffold audit. `docs/reviews/README.md` was intentionally
not edited, so this package is not in its index yet.

## Record

- **Agent / model:** DeepSeek V4.1 Flash (`opencode-go/deepseek-v4.1-flash`),
  implementation sub-agent for the release-hardening task.
- **Commit:** `9b9492d` — "[deepseek] Harden release version and prerelease
  checks", parent `70be4a4`. The record was first committed as `7b75444`;
  it was amended once to add this hash line, so the commit under review is
  `9b9492d` (same message and parent, trees differ only in this paragraph).
  Other agents committed core/docs work while this task ran, which is why the
  parent is `70be4a4` and not the `9900a1b` seen at the start; one concurrent
  maintenance commit landed between the two and was replayed after `9b9492d`
  with its content unchanged. This parent hash was corrected in a follow-up
  commit.
- **Files:**
  - `scripts/release.ps1` — preflight, prerelease flag, pre-upload budget
    gate, exact installer match
  - `scripts/version.ps1` — five version fields and the offline `Cargo.lock`
    refresh
  - `.github/actions/release/action.yml` — mirrored version check,
    prerelease flag, budget gate, installer match
  - `README.md` — release steps, prereleases, budget, preflight
  - `CLAUDE.md` — the release rule bullet
  - `docs/reviews/2026-11-17-release-hardening.md` — this record
  - Not touched: `.github/workflows/release.yml` (the composite action carries
    the changes; the self-hosted job and hosted fallback are unchanged),
    `crates/relay-core`, `src/`, `src-tauri/src/`, `design/`,
    `docs/reviews/README.md`.

## What changed

### `release.ps1` preflight (before any install, build or upload)

1. **Clean tree** (unchanged): `git status --porcelain` must be empty.
2. **All version fields equal the release argument.** Five sources are read:
   - `package.json` (JSON, top-level `version`)
   - `src-tauri/tauri.conf.json` (JSON, top-level `version`)
   - `src-tauri/Cargo.toml` `[package].version`
   - `crates/relay-core/Cargo.toml` `[package].version`
   - `Cargo.toml` `[workspace.package].version` when the table exists

   A missing required value fails. A mismatch prints every offending field
   with its actual value, for example:

   ```
   Version mismatch for v0.1.0: package.json is 0.1.1, expected 0.1.0;
   Cargo.toml [workspace.package] is 0.1.1, expected 0.1.0. Run
   scripts/version.ps1 -Version 0.1.0, commit the bump, then move the tag.
   ```

3. **Tag exists locally** (unchanged): `git tag --list v<version>`.
4. **`gh` is present and cannot clobber a release:**
   - `Get-Command gh` failing throws: `gh (GitHub CLI) is not on PATH. ... The
     release was not built.`
   - The repository is resolved from `GH_REPO`, else from
     `git remote get-url origin` (SSH or HTTPS GitHub URL). Neither present
     throws: `Cannot resolve the GitHub repository: no 'origin' remote and
     GH_REPO is not set. ...`
   - `gh release view <tag> --repo <repo>` is the read-only probe:
     - exit 0 → `Release <tag> already exists on <repo>. Refusing to clobber
       it: delete that release or release a new version.`
     - stderr/stdout matches `release not found` → proceed
     - any other result → throw with the probe output (auth/network/API
       failure must not be treated as "no release")
   - `gh auth status` is run for the record. Its exit code is tolerated only
     when the probe above succeeded: this machine's `gh` exits 1 because of an
     invalid secondary account (`gustavo-linck`) while the active account
     (`uFleck`) can read the repo. A warning says so; a broken active account
     fails through the probe instead.
5. **Signing key exists** (unchanged).

`-DryRun` keeps every check but turns each failure into a warning that says a
real run would stop, so the planned steps can still be inspected. Nothing is
built, written or uploaded in a dry run.

### Prerelease handling

A version with a prerelease suffix (a `-` before any `+build`, e.g.
`0.2.0-beta.1`) now:

- prints `Prerelease: GitHub gets --prerelease; /releases/latest stays on the
  newest stable release.`;
- passes `--prerelease` to `gh release create`;
- uses the title `Relay v<version> (pre-release)`;
- still writes and uploads `latest.json`. GitHub excludes prereleases from
  `/releases/latest`, so the updater endpoint keeps serving the newest stable
  release. Testers install the beta manually.

The same logic is in `action.yml` (`IS_PRERELEASE` from `GITHUB_REF_NAME`).

### Budget gate before upload

- After install + build, the script prints
  `Budget gate: <N>s at the end of the build (install + build; upload
  excluded).`
- `> 300s` warns about the 5:00 target.
- `> 420s` throws **before** installer discovery, `latest.json` and
  `gh release create`, unless `-ForceOverBudget` is passed (then it warns and
  continues). The message says nothing was published.
- Per-step timings are unchanged. The final line now says
  `Total incl. upload: ...` and `Budget gate measured <N>s before upload; the
  upload time is excluded from the 420s gate.`
- The old post-publish 420s throw was removed, so a slow upload can no longer
  turn an already-published release red.

### Installer discovery

- Requires an exact `<product>_<version>_<arch>-setup.exe` (pattern
  `*_<version>_*-setup.exe`) and its `.sig`.
- Zero matches throws and lists the `.exe` files that were found.
- More than one match throws as ambiguous.
- The "newest `.exe`" fallback was removed from both `release.ps1` and
  `action.yml`.

### `version.ps1`

- Updates the same five fields as the preflight. `[workspace.package].version`
  is optional: a missing table prints a skip line; a table without a version
  line warns.
- The JSON files still go through Node with `JSON.stringify(json, null, 2)`,
  so key order and formatting are stable.
- After a Cargo version actually changes it runs
  `cargo metadata --offline --format-version 1` to refresh `Cargo.lock` without
  the network. On failure it warns and prints the first 8 lines of Cargo's
  error. When no Cargo version changed it prints `Cargo versions unchanged;
  Cargo.lock not touched.` and runs nothing.
- Idempotence is preserved: a second run with the same version changes no
  file.

### `action.yml`

- New step **Verify version fields match tag** runs before **Set version from
  tag** (which mutates the files) and checks the same five fields against
  `GITHUB_REF_NAME`. The mismatch message names every field.
- **Set version from tag** now also writes `[workspace.package].version` when
  the table exists, and exports `IS_PRERELEASE`.
- New step **Budget gate (pre-upload)** runs after the build: warns above
  300s; fails above 420s only on `self-hosted` (the hosted fallback is not
  held to it, as before); nothing is published when it fails.
- **Write latest.json** uses the exact installer match and still writes
  `latest.json` for prereleases.
- **Publish GitHub release** adds `--prerelease` and the `(pre-release)` title;
  `--verify-tag` and `--generate-notes` are unchanged.
- **Total time** still prints the total including upload but no longer fails;
  it states that the gate uses the pre-upload time.
- The self-hosted job, hosted fallback, caches and secrets are untouched.

## Commands and results

Run on Windows, `pwsh` 7.6.10. Timings use the machine's decimal comma.

### 1. `version.ps1` bump, restore and idempotence

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/version.ps1 0.1.1
```

Exit 0. All five fields set to `0.1.1`. The lock refresh reported:

```
Refreshing Cargo.lock (offline, no network)...
WARNING: cargo metadata --offline failed (exit 101). No network was used; Cargo
may still have rewritten the local package entries in Cargo.lock, so check
'git diff Cargo.lock' before committing. The next build will complete the lock.
error: failed to download `async-executor v1.14.1`
Caused by:
  attempting to make an HTTP request, but --offline was specified
```

`git diff Cargo.lock` showed only the two local entries (`relay` and
`relay-core`) moved to `0.1.1`; Cargo wrote those before failing on a registry
cache miss. No network request was made (`--offline`). The version was then
restored with `scripts/version.ps1 0.1.0`.

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/version.ps1 0.1.0
```

Exit 0, second run:

```
  package.json -> 0.1.0
  src-tauri/tauri.conf.json -> 0.1.0
  src-tauri/Cargo.toml [package] already 0.1.0
  crates/relay-core/Cargo.toml [package] already 0.1.0
  Cargo.toml [workspace.package] already 0.1.0
Cargo versions unchanged; Cargo.lock not touched.
Version set to 0.1.0.
```

`git status --porcelain` for `package.json`, `src-tauri/tauri.conf.json`,
`src-tauri/Cargo.toml`, `crates/relay-core/Cargo.toml`, `Cargo.toml` and
`Cargo.lock` was empty after the two 0.1.0 runs. All five fields and both
`Cargo.lock` entries read `0.1.0`.

### 2. `release.ps1` dry run

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/release.ps1 0.1.0 -DryRun
```

```
Relay release v0.1.0
DRY RUN: nothing is built, written or uploaded.
WARNING: Working tree is not clean; a real run would stop here.
Version fields match v0.1.0 (5 checked).
WARNING: Tag v0.1.0 does not exist.
WARNING: Cannot resolve the GitHub repository: no 'origin' remote and GH_REPO is not set. A real run would stop here.

Would run:
  pnpm install --frozen-lockfile
  pnpm tauri build
  check budget gate: warn above 300s, fail above 420s before upload unless -ForceOverBudget (upload excluded)
  require <product>_0.1.0_<arch>-setup.exe and its .sig (no newest-.exe fallback)
  write latest.json for v0.1.0
  gh release create v0.1.0 <installer>.exe <installer>.exe.sig latest.json --verify-tag --title "Relay v0.1.0" --generate-notes
Total: 0,3s
```

The dirty tree is the UI work in `src/` from another task; this checkout has no
`origin` (audit finding 26), so a real run stops at the repository check until
`origin` or `GH_REPO` exists.

```powershell
$env:GH_REPO='uFleck/relay'
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/release.ps1 0.1.0 -DryRun
```

Adds, in place of the repository warning:

```
No existing release for v0.1.0 on uFleck/relay.
WARNING: gh auth status exits 1 (often an invalid secondary account); the
release probe worked, so the active account is usable. Run 'gh auth status' to
inspect.
```

Read-only; no release was created.

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/release.ps1 0.2.0-beta.1 -DryRun
```

```
Relay release v0.2.0-beta.1
Prerelease: GitHub gets --prerelease; /releases/latest stays on the newest stable release.
...
WARNING: Version mismatch for v0.2.0-beta.1: package.json is 0.1.0, expected 0.2.0-beta.1; ...
...
  require <product>_0.2.0-beta.1_<arch>-setup.exe and its .sig (no newest-.exe fallback)
  write latest.json for v0.2.0-beta.1
  gh release create v0.2.0-beta.1 <installer>.exe <installer>.exe.sig latest.json --verify-tag --title "Relay v0.2.0-beta.1 (pre-release)" --generate-notes --prerelease
```

### 3. Negative preflight tests (throwaway clone of `9900a1b` + this script)

A `git clone` of the repo in `%TEMP%`, with the new `release.ps1` committed so
the tree stays clean. No build was reached in any case.

| Case | Setup | Result |
| --- | --- | --- |
| A | `Cargo.toml [workspace.package]` and `package.json` set to `0.1.1`, clean tree, tag `v0.1.0` | exit 1: `Version mismatch for v0.1.0: package.json is 0.1.1, expected 0.1.0; Cargo.toml [workspace.package] is 0.1.1, expected 0.1.0.` `pnpm tauri build` never ran |
| B | versions match, no tag | exit 1: `Tag v0.1.0 does not exist...`; no build |
| C | versions match, tag created, no `origin`, no `GH_REPO` | exit 1: `Cannot resolve the GitHub repository...`; no build |

### 4. End-to-end harness with fake `pnpm` and `gh` (throwaway clone)

`pnpm.cmd` writes a fake bundle and `.sig`; `gh.cmd` answers `auth status`,
`release view` and logs the `release create` arguments. The real script ran
unchanged.

| Case | Setup | Result |
| --- | --- | --- |
| 1 | bundle `Relay_0.9.9_x64-setup.exe` only | exit 1 at discovery: `No NSIS installer for 0.1.0 ... Found: Relay_0.9.9_x64-setup.exe`; no `gh` call |
| 2 | bundle `Relay_0.1.0_x64-setup.exe` + `.sig` | exit 0; `Budget gate: 0,5s ...` printed before discovery; `latest.json` version `0.1.0`; `gh release create v0.1.0 ... --verify-tag --title "Relay v0.1.0" --generate-notes` |
| 3 | `version.ps1 0.2.0-beta.1`, tag, matching bundle | exit 0; `--title "Relay v0.2.0-beta.1 (pre-release)" ... --prerelease`; `latest.json` version `0.2.0-beta.1` |

### 5. Existing-release and budget branches (second harness)

| Case | Setup | Result |
| --- | --- | --- |
| D | fake `gh release view` exits 0 | exit 1: `Release v0.1.0 already exists on uFleck/relay. Refusing to clobber it...`; no build |
| E | threshold in the throwaway copy patched from 420 to 0.01, no flag | exit 1 after the build at the gate: `... over the 420s budget (upload excluded). Nothing was published.`; no `gh` call |
| F | same patched copy, `-ForceOverBudget` | warning, then publish; exit 0; `gh release create` called |

Case E/F exercise the gate branch and the override, not the literal 420s
value; the numbers 300/420 are enforced by static inspection of the final
script.

### 6. Composite action steps executed from the YAML

Each `run:` block was extracted from `action.yml` and executed with synthetic
`GITHUB_*` variables.

- `Verify version fields match tag` with `GITHUB_REF_NAME=v0.1.0`: exit 0,
  `Version fields match v0.1.0 (5 checked).`
- Same step with `v0.2.0-beta.1`: exit 1 and all five mismatches named.
- `Set version from tag` with `v0.2.0-beta.1` in a temp copy: exit 0; all five
  files contain `0.2.0-beta.1`; `GITHUB_ENV` got
  `APP_VERSION=0.2.0-beta.1` and `IS_PRERELEASE=true`.
- `Write latest.json` with `APP_VERSION=0.1.0` against the real
  `target/release/bundle/nsis` bundle: found `Relay_0.1.0_x64-setup.exe` and
  its `.sig`, wrote `latest.json`, exported `EXE_PATH`/`SIG_PATH`.
- Same step with `APP_VERSION=9.9.9`: exit 1, `No NSIS installer for 9.9.9 ...
  Found: Relay_0.1.0_x64-setup.exe`. No fallback.

### 7. Static validation

- PowerShell AST parse (`[System.Management.Automation.Language.Parser]`) —
  `scripts/release.ps1`, `scripts/version.ps1`,
  `scripts/generate-signing-key.ps1`: **PARSE OK** (0 errors each).
- `python -c "import yaml; ..."` (PyYAML) loads
  `.github/actions/release/action.yml`, `.github/workflows/release.yml` and
  `.github/workflows/ci.yml`: **YAML OK**.
- All 11 `pwsh` `run:` blocks in `action.yml` parse as PowerShell: **OK**.

No full Tauri build, tag, release or GitHub Actions run was performed.

## Known risks

- **Budget threshold not observed live.** The 420s failure was exercised with
  the threshold patched in a throwaway copy; the real run would need a 7-minute
  fake build. The literal 300/420 values are static-inspection only.
- **`gh release view` text matching.** The absence check depends on the
  English `release not found` string. A localized or changed `gh` output
  fails safe (the run stops with the raw output) but blocks releases until the
  matcher is updated.
- **`gh auth status` tolerance.** A non-zero exit is downgraded to a warning
  only when the repository probe succeeded. If `gh auth status` starts failing
  for the active account while a cached token still reads releases, the run
  would proceed on the probe's evidence.
- **No `origin` in this checkout.** The real preflight still cannot run here
  until `git remote add origin` or `GH_REPO` is provided (audit finding 26
  remains operational, not code).
- **Probe/publish race.** Nothing prevents another actor from creating the
  release between `gh release view` and `gh release create`; `gh` would then
  fail the create, after the build.
- **`Cargo.lock` partial write.** On this machine `cargo metadata --offline`
  updates the two local package entries and then fails on
  `async-executor v1.14.1` (registry cache miss). The bump continues with a
  warning and the lock is consistent for the bumped version, but a reviewer
  should confirm the wording is enough for someone who runs `git add -A`.
- **Prerelease `latest.json`.** It is still uploaded as the task required.
  GitHub's `/releases/latest` excludes prereleases; if someone manually marks
  the beta as "latest" in the web UI, the updater would serve it. `gh` has no
  flag for that and the scripts do not touch it.
- **Action ordering.** The version-field check runs after toolchain setup
  (pnpm/Node/Rust) but before install/build, so a mismatch costs a few seconds,
  not a build.
- **Full CI path unverified.** No GitHub Actions run and no real
  `gh release create` happened; the composite steps were executed locally with
  synthetic environment variables.
- **Signing secret pairing** is still unverified (scaffold audit risk); this
  change does not touch it.

## Incident — destructive git reset (2026-11-17)

While reconciling a concurrent commit, this package ran `git reset --hard
7b75444`. That discarded the uncommitted UI edits to `src/App.tsx`,
`src/main.tsx` and `src/styles/base.css`; the untracked UI files were not
removed. The three files were recovered from T3 Code's recorded file-change
diffs (`%USERPROFILE%\.t3\userdata\statev2.sqlite`), line endings were
restored to LF, and `pnpm typecheck` + `pnpm build` pass. A seeded 5-task
visual test after recovery renders the same Today groups, claims, unacked
change dot and links as before the reset; the demo rows were removed
afterwards. The worktree again shows the same 3 modified + 8 untracked UI
paths.

History was repaired: the original index commit `ac21440` was replayed as
`a819c07` with the same `docs/HANDOFF.md` / `docs/reviews/README.md` content;
release commit `9b9492d` carries the release changes; `9ae219a` corrects this
record's parent. `7b75444` is the pre-amend release commit and is not on the
branch.

Opus must spot-check the recovered UI files and the release commit, and confirm
no UI behavior was lost. New rule in `CLAUDE.md`: never run destructive git
commands while another agent has uncommitted work.

## Review checklist for Opus

- [ ] Confirm the five version sources are complete and that
      `[workspace.package]` may be skipped when absent without hiding a
      mismatch.
- [ ] Confirm the `gh` probe logic: exit 0 refuses, `release not found`
      proceeds, anything else fails; and that the auth-status downgrade is
      acceptable.
- [ ] Confirm `--prerelease` plus the `(pre-release)` title, and that keeping
      `latest.json` on prereleases matches the intended updater policy.
- [ ] Confirm the budget gate sits after the build and before discovery,
      `latest.json` and `gh release create`, that upload is excluded, and that
      no post-publish failure remains.
- [ ] Confirm `-ForceOverBudget` is the only override and that the warning
      text is clear.
- [ ] Confirm the exact installer pattern matches Tauri's NSIS naming for
      stable and prerelease versions and that ambiguity fails.
- [ ] Confirm `version.ps1` is idempotent, only touches the intended lines,
      and that the offline-lock fallback wording is accurate.
- [ ] Confirm `action.yml` checks before it rewrites, mirrors the local
      preflight, and leaves the self-hosted/hosted structure intact.
- [ ] Confirm `README.md` describes the real order (tag pushed before
      `release.ps1`), the preflight, prereleases and the budget gate.
- [ ] Confirm no key, password or token is printed by any new path.

## Opus verdict

<!-- Opus: write the verdict here after reviewing the commit in this record. -->
