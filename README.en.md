# gh-updater

English | [日本語](README.md)

A resident updater that watches the executables (and `.app.dmg` files) uploaded to GitHub Releases, verifies their SHA-256, places them in the given folders, and stops and starts the services, all in one process. The settings are TOML.

- Supported: Windows x64 / macOS arm64 / Linux x64, arm64 and riscv64 (musl)
- One file can be copied to several folders, and several targets are watched together
- Restart methods: `exec` / `docker-compose` / `launchd` / `terminal-command` / `windows-process` / `reboot`
- A change is detected by comparing the `digest` (sha256) of the asset with the state file (it works even when the tag is fixed)

- Self-update: with `[self_update]`, gh-updater updates itself too (after every target has been updated; it checks that the new file works with `--version` before replacing it, then restarts). If an update is interrupted, the service is started again at the next start

## Usage

```bash
gh-updater --config updater.toml          # resident (checks every interval)
gh-updater --config updater.toml --once   # check and update once
gh-updater --config updater.toml --check  # check only (no update)
```

Without `--config`, `config.toml` is searched in the current directory first, then in the directory of the executable (only the one that is found is read).

See `updater.*.toml` for examples (the comment at the top explains the structure). Templates to run it as a service are in `service/` (systemd / launchd / Task Scheduler / NanoKVM).

## Build

```bash
cargo build --release   # for this machine
./build-cross.sh        # 5 platforms (cross + Docker). The output is in dist/
```

`updater.minecraft.example.toml` is a design note for the automatic update of Minecraft jars in the future, and the current version cannot read it.
