# What stays stable

diskdiet runs unattended (the background service, cron jobs, scripts that read
`--json`), so some names must not change under people. This page lists them.
`tests/compat.rs` checks every item here on each pull request.

diskdiet is still **beta**. Until 1.0, the rules are:

- Nothing below is removed or renamed without first working under both names
  for at least one release, with a warning that says what to use instead.
- New commands, flags, config keys and JSON fields may be added at any time.
  Scripts should ignore JSON fields they don't know.
- Anything **not** listed here (the picker layout, colors, wording of
  messages, the desktop app's screens) can change in any release.

From 1.0 on, changing anything listed here needs a major version.

## Commands and flags

| Command | Flags |
|---|---|
| `diskdiet scan [paths]` | `--json` `--older-than` `--min-size` `--no-projects` `--no-caches` `--no-docker` `--big` `--no-git` |
| `diskdiet clean [paths]` | the `scan` filters, plus `-y/--yes` `--dry-run` `--trash` `--tui` `--wipe-caches` `--docker-all` `--docker-volumes` |
| `diskdiet big [paths]` | `--min-size` and the `clean` flags |
| `diskdiet docker` | `--prune` and the `clean` flags |
| `diskdiet undo` | `--list` `--id` `-y/--yes` |
| `diskdiet alert` | `--below` `--no-notify` |
| `diskdiet stats` | `--json` |
| `diskdiet config init / path / show` | |
| `diskdiet service install [paths]` | `--every` `--older-than` `--with-docker` `--no-caches` `--trash` `--alert-below` `--alert-every` |
| `diskdiet service uninstall / status` | |

Sizes (`10MB`, `1.5G`) and durations (`30d`, `2w`, `12h`) keep their formats.
Exit status is `0` on success and `1` on error.

## Config file

`~/.config/diskdiet/config.toml` (or `$XDG_CONFIG_HOME/diskdiet/config.toml`).

Keys: `paths` `protected` `exclude` `older_than` `min_size` `trash` `picker`
`smart_caches` `skip_caches` `big_min_size` `big_paths` `update_check`, and `[[rules]]` with
`dir` `markers` `kind`.

Unknown keys are an error on purpose, so a typo never silently turns off a
safety setting.

## JSON output

`diskdiet scan --json`: `version` `free_space` `reclaimable` `items[]`.
Each item has `category` (`project` `cache` `docker` `big`) `kind` `label`
`path` `size` (bytes) `age_days` `note` `default_selected` `git`.

`diskdiet stats --json`: `total_freed` `total_trashed` `items_removed` `runs`
`first_run` `last_run` `history[]` (`at` `freed` `trashed` `items` `source`).

## Files and background jobs

| What | Where |
|---|---|
| Stats | `<data dir>/diskdiet/stats.json` |
| Undo log | `<data dir>/diskdiet/undo.json` |
| Update check cache | `<data dir>/diskdiet/update.json` |
| macOS jobs | launchd agents `dev.diskdiet.agent` (clean) and `dev.diskdiet.alert` |
| Linux jobs | systemd user timers `diskdiet.timer` and `diskdiet-alert.timer` |

`<data dir>` is `~/Library/Application Support` on macOS and
`~/.local/share` on Linux.

## The safety rules

These are promises, not just defaults, and `tests/safety.rs` checks them:

- Symlinks are never followed, while scanning or deleting.
- A folder only counts as build output when its build file sits next to it.
- Projects with uncommitted git changes are never removed by `--yes` or the service.
- Big files and the Trash are never removed automatically. Unused tagged images and Docker volumes only with `--docker-all` / `--docker-volumes`.
- `protected` and `exclude` paths are never listed.
