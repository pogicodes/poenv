# poenv

Your configs and secrets in one encrypted vault on your machine. Switch a repo's `.env` between environments with one command, and let AI agents use your secrets without poenv ever showing them the values.

```bash
cd my-app
poenv init            # import the env file (.env.local in Next.js, else .env) into the vault
poenv use staging     # the env file is now on staging
poenv add OPENAI_API_KEY   # pull in a key you share across apps
```

- **One local vault**: a single encrypted file, opened by a key from your recovery phrase. No password to type, no server, no account.
- **Whole-folder switching**: `poenv use staging` rewrites the env file itself, so Next.js, Vite, Prisma, Docker Compose, Python and everything else that reads `.env` sees the new env.
- **Shared keys**: set `OPENAI_API_KEY` once, use it from every app, rotate it in one place.
- **Agent-safe**: agents can list key names, switch envs and run commands, but poenv never prints a value to them or lets them switch to a protected env like `prod`.
- **Any language**: it writes a standard `.env` file, or injects variables with `poenv run -- <cmd>`.

> **AI agents:** to install and set up poenv, follow [INSTALL.md](INSTALL.md). Every command, its `--json` output and exit codes: [docs/commands.md](docs/commands.md). To give every agent on your machine a `/poenv` command: `npx skills@1.7.6 add pogicodes/poenv-skill`.

## Install

macOS and Linux:

```bash
curl -fsSL https://poenv.pogi.codes/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://poenv.pogi.codes/install.ps1 | iex
```

Then:

```bash
poenv doctor          # checks your setup and tells you exactly what to fix
cd your-repo && poenv init
```

The first `poenv init` creates your vault and shows a **24-word recovery phrase, once**. Save it in your password manager: it opens your backups, and rebuilds this machine's key if it's ever lost. Installer options, updating and uninstalling: [docs/setup.md](docs/setup.md).

## See it in action

### Set up a repo

`poenv init` creates your vault on first run (and shows its recovery phrase), then moves the repo's env file into it.

![poenv init: save the recovery phrase, import .env.local](docs/demo/init.gif)

### Switch environments

`poenv use staging` rewrites the env file, so every tool that reads it is now on staging.

![poenv use: switch .env.local from local to staging](docs/demo/use.gif)

### Share a key across apps

`init` spots a value another app already has and offers to share it. Rotating it later rewrites every repo that uses it.

![poenv shared keys: blog shares OPENAI_API_KEY with shop, then one rotation updates both](docs/demo/shared.gif)

### Browse and edit in the TUI

Run `poenv` on its own. Values stay masked until you press `v`, and `u` switches the env file.

![poenv TUI: browse keys, reveal a value, switch the env file to staging](docs/demo/tui.gif)

### Run a command with an env

`poenv run staging -- <cmd>` injects the env into one command and leaves the file alone.

![poenv run: inject staging and prod into a command without changing .env.local](docs/demo/run.gif)

### What AI agents can do

In an agent's shell, poenv lists key names and switches envs, but won't print values or switch to protected envs. Values in `poenv run` output are shown as `***`, and `poenv values run` keeps them out of the env file entirely ([details](docs/env-file.md#keeping-values-out-of-the-file)). `poenv init` adds a short section to `AGENTS.md` / `CLAUDE.md` so agents know how. Installing for an agent: [INSTALL.md](INSTALL.md).

For a `/poenv` command in Claude Code, Codex, Cursor and other agents, install the skill: `npx skills@1.7.6 add pogicodes/poenv-skill` ([details](docs/setup.md#agent-skill-and-poenv)).

![poenv for agents: ls works, get and use prod are refused, use staging works](docs/demo/agents.gif)

## Docs

- [Commands](docs/commands.md): every command, the TUI keys, `--json` and exit codes
- [How the env file is written](docs/env-file.md): the managed block, frameworks and formats, Java and Spring Boot
- [Security model](docs/security.md): what the encryption protects, and what agents can reach
- [Install and setup](docs/setup.md): installer options, first run, update and uninstall
- [SPEC.md](SPEC.md): the full design

## Build from source

```bash
cargo test                       # unit, CLI end-to-end and TUI tests
cargo build --release            # target/release/poenv
```

The check against the real Node dotenv, Next.js, Expo, python-dotenv, java.util.Properties and Spring Boot loaders runs in CI. To run it locally: `npm ci --prefix tests/loaders && pip install -r tests/loaders/requirements.txt`, put the Spring Boot jars in `tests/loaders/jars` (see `.github/workflows/test.yml`), then `POENV_TEST_LOADERS=1 cargo test --test loaders`. Java needs to be installed.
