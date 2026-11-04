# Install and setup

macOS and Linux:

```bash
curl -fsSL https://poenv.pogi.codes/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://poenv.pogi.codes/install.ps1 | iex
```

## Installer options

No sudo or admin rights: it installs to `~/.local/bin` (Windows: `%LOCALAPPDATA%\poenv\bin`), checks the SHA-256, and adds that folder to your PATH. Options for `install.sh`: `--version X`, `--dir PATH`, `--no-modify-path`, passed as `curl -fsSL https://poenv.pogi.codes/install.sh | sh -s -- --version X`. Both installers also read the env vars `POENV_VERSION`, `POENV_INSTALL_DIR` and `POENV_NO_MODIFY_PATH` (the only way to set options on Windows). Or download a binary from [Releases](https://github.com/pogicodes/poenv/releases).

## First run

```bash
poenv doctor          # checks your setup and tells you exactly what to fix
cd your-repo && poenv init
```

The first `poenv init` creates your vault and shows a **24-word recovery phrase, once**. Save the phrase in your password manager: it opens your backups (`poenv backup` / `poenv restore`), and rebuilds this machine's key if it's ever lost (`poenv recover`). There's no master password: poenv opens the vault on its own. See the [security model](security.md) for what that means.

## Agent skill and `/poenv`

[`skills/poenv`](../skills/poenv/SKILL.md) is an [Agent Skill](https://agentskills.io): it teaches a coding agent to install poenv, set up a repo, switch envs and run commands without seeing values. Install it into every agent on your machine:

```bash
npx skills add pogicodes/poenv
```

Then type `/poenv` (or `/poenv use staging`, `/poenv setup`) in Claude Code or Cursor, or `$poenv` in Codex. Agents also pick it up on their own when you ask about env vars.

## Update

Run the installer again. It replaces the binary in place and leaves your vault, PATH and repos alone:

```bash
curl -fsSL https://poenv.pogi.codes/install.sh | sh
```

```powershell
irm https://poenv.pogi.codes/install.ps1 | iex
```

`poenv --version` shows what you have. Pass `--version X` (or `POENV_VERSION`) to pin or roll back to a release.

## Uninstall

1. Delete the binary: `rm ~/.local/bin/poenv` (Windows: `Remove-Item "$env:LOCALAPPDATA\poenv\bin\poenv.exe"`), or wherever `--dir` put it.
2. Remove the PATH line ending in `# added by poenv installer` from `~/.zshrc`, `~/.bashrc`, `~/.bash_profile` or `~/.profile` (fish: `fish_add_path`; Windows: remove `%LOCALAPPDATA%\poenv\bin` from your user `Path`).
3. Optional, and permanent: delete the vault in `~/.poenv` (Windows: `%APPDATA%\poenv`). Every secret in it is gone unless you ran `poenv backup` first.

Your repos keep working: each env file still holds the values of the env it was last switched to. Delete `.poenv` and the poenv section in `AGENTS.md` / `CLAUDE.md` if you don't want them.
