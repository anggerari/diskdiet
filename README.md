# diskdiet 🍔➡️🥗

> Put your storage on a diet.

Your storage is not full of photos. It's full of `node_modules`, Rust `target/` folders, Flutter builds, npm/pnpm/yarn caches, Xcode DerivedData and Docker images you forgot about. `diskdiet` is a fast Rust CLI that finds all of it, shows you how much each item costs, and lets you pick what to delete.

```text
$ diskdiet clean ~/code

diskdiet clean
21.4 GB free on disk. Found 84 item(s), ~58.3 GB reclaimable.

   #  sel  type          age       size  path
   1  [x]  Rust           3d     6.8 GB  ~/code/my-rust-api/target
   2  [x]  cache           -     5.1 GB  Go build cache ~/Library/Caches/go-build
   3  [x]  Node          21d     2.4 GB  ~/code/old-landing-page/node_modules
   4  [x]  Flutter       40d     2.1 GB  ~/code/mobile-app/build
   5  [ ]  docker image   5w   980.0 MB  postgres:15 · unused
   6  [x]  Next.js        1d   870.2 MB  ~/code/dashboard/.next
   7  [ ]  docker vol      -   415.0 MB  shop_db_data · unused, may hold data
   8  [ ]  docker ctr     2w    20.0 KB  shop-db-1 (postgres:15) · Exited (0) 2 weeks ago
  ...

  Selected: 61 item(s), ~41.7 GB

  Toggle by number (e.g. "1 3 8" or "2-6"), a = all, n = none, Enter = confirm, q = quit:
```

## Why diskdiet exists

Developers lose storage without noticing. Every `npm install` adds hundreds of megabytes of `node_modules`. Every `cargo build` adds gigabytes to `target/`. Every `docker build` leaves old images behind. Package managers keep a cache of everything they have ever downloaded. Do that across a dozen projects, a few git worktrees and a year of experiments, and a 256 GB or 512 GB laptop runs out of space. Then you get *"Your disk is almost full"*, slow builds and Docker refusing to start.

The cleanup knowledge is scattered. `npm cache clean`, `docker system prune`, `cargo clean`, deleting `~/Library/Developer/Xcode/DerivedData`, finding which of 40 `node_modules` folders belong to projects you abandoned months ago… Each tool has its own command, and none of them shows you the whole picture.

**diskdiet puts it all on one screen:**

- **See**: one scan lists every project artifact, every package-manager cache and every Docker image, container and volume, sorted by size.
- **Choose**: you decide what goes. Nothing is deleted until you pick it and confirm.
- **Stay lean**: an optional background service cleans projects you haven't touched in weeks, so storage never fills up again.

Everything it removes can be regenerated (`npm install`, `cargo build`, `docker pull` and so on). You trade a few minutes of re-downloading for gigabytes back today.

## Features

- **Finds project junk safely.** A folder only counts when its build system is present next to it. `target/` needs a `Cargo.toml` or `pom.xml`, `build/` needs Gradle or Flutter files, `node_modules/` needs a `package.json`. A hand-made folder that happens to be called `build` stays put.
- **Clears global caches** for npm, Yarn, pnpm, Bun, Cargo, pip, uv, Go, Gradle, Maven, Composer, Xcode DerivedData, simulators, CocoaPods, Homebrew, Playwright, Puppeteer, Electron, JetBrains and the Trash.
- **Scans Docker item by item.** Every image (with its real unique size and whether a container uses it), every stopped container, every unused volume and the build cache gets its own row you can pick. Running containers are never touched.
- **Numbered toggle picker.** Type numbers or ranges to choose. Nothing is deleted until you confirm.
- **Age filter**: `--older-than 30d` only touches projects you haven't worked on in a while.
- **Background service**: a weekly launchd agent (macOS) or systemd user timer (Linux).
- **Fast**: folder sizes are measured in parallel, and hard links are counted once.

### What it looks for

| Ecosystem | Folders |
|---|---|
| Node / JS | `node_modules`, `.next`, `.nuxt`, `.output`, `.svelte-kit`, `.turbo`, `.parcel-cache`, `.angular` |
| Rust / JVM | `target` (Cargo, Maven), `build` + `.gradle` (Gradle) |
| Mobile | Flutter `build` + `.dart_tool`, `Pods`, SwiftPM `.build` |
| Python | `.venv`, `venv`, `__pycache__`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache` |
| Others | Zig, Haskell Stack, Elixir, Elm, Terraform |
| Docker | images, stopped containers, unused volumes, build cache |

## Install

Requires a Rust toolchain ([rustup.rs](https://rustup.rs)).

```sh
git clone https://github.com/anggerari/diskdiet
cd diskdiet
make install          # cargo install --path .
```

## Usage

```sh
diskdiet scan ~/code                      # read-only report
diskdiet clean ~/code                     # pick and delete
diskdiet clean ~ --older-than 30d         # only projects untouched for 30+ days
diskdiet clean --no-caches --no-docker    # only project folders in the current dir
diskdiet clean ~/code --dry-run           # preview what --yes would delete
diskdiet docker                           # pick Docker images / containers / volumes
diskdiet docker --prune --all             # bulk prune, like docker system prune -a
```

| Flag | Meaning |
|---|---|
| `--older-than 30d` | Only projects untouched for at least this long (`s`, `m`, `h`, `d`, `w`) |
| `--min-size 100MB` | Hide anything smaller (default `1MB`) |
| `--no-projects` / `--no-caches` / `--no-docker` | Skip a category |
| `-y, --yes` | Delete without asking (see below for what Docker items are included) |
| `--docker-all` | With `--yes`: also remove tagged images no container uses |
| `--docker-volumes` | With `--yes`: also remove unused volumes (⚠️ may contain database data) |

### Docker safety levels

| Item | Picker default | Removed by `--yes` |
|---|---|---|
| Dangling images, build cache | selected | yes |
| Stopped containers | not selected | yes |
| Unused tagged images | not selected | only with `--docker-all` |
| Unused volumes | not selected | only with `--docker-volumes` |
| Images used by a container | not selected | never |
| Running containers | not listed | never |

The **Trash** is also left unselected by default.

### Background service

```sh
diskdiet service install ~/code                     # weekly, projects untouched for 14d + caches
diskdiet service install ~/code --every 3d --older-than 30d --with-docker
diskdiet service status
diskdiet service uninstall
```

On macOS the service is a launchd agent and logs to `~/Library/Logs/diskdiet.log`. On Linux it is a systemd user timer, so use `journalctl --user -u diskdiet.service` to see its logs.

## Make targets

```text
make scan       # read-only report of $HOME (override: PATHS=~/code)
make diet       # interactive cleanup
make docker     # pick Docker items to remove
make install    # install to ~/.cargo/bin
make service-install PATHS=~/code
make test lint fmt
```

## Notes

- **pnpm**: each project's `node_modules` is hard-linked from the shared store, so deleting one frees less than its listed size. Clearing the **pnpm store** is what actually frees that space.
- **Docker Desktop (macOS)** keeps images in a VM disk file. After pruning, the file usually shrinks on its own. If it doesn't, use Docker Desktop → Troubleshoot → *Clean / Purge data*.
- On macOS, emptying `~/.Trash` from a terminal may require *Full Disk Access* for that terminal.
- Everything diskdiet deletes can be regenerated. Commit your work anyway.

## License

MIT
