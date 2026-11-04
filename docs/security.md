# Security model

Your vault is a single AES-256-GCM encrypted file, `~/.poenv/store.enc` (Windows: `%APPDATA%\poenv\store.enc`). Its key comes from your 24-word recovery phrase and is kept next to it in `keys.json` (owner-only), so poenv opens the vault without asking for anything. There's no master password and nothing to unlock.

What that protects, and what it doesn't:

- **Backups**: a file from `poenv backup` is the encrypted vault alone. It opens only with your recovery phrase, so you can keep it anywhere.
- **Programs running as you**, including AI agents, can read `keys.json`, and so every env. A password wouldn't change that in practice: agents need the vault open while they work, and the env file each repo is on already holds its values in plain text.
- **A copy of the whole `~/.poenv` folder** (a Time Machine backup, a synced folder) holds both the vault and its key. Encrypt those backups at the OS level (for Time Machine: "Encrypt backups"), or keep `~/.poenv` out of synced folders.

What keeps values away from agents is how poenv behaves, not encryption. These rules stop leaks by mistake, not an agent that sets out to read files it can reach:

- Agents can list key names, switch envs and run commands with secrets injected, but poenv never prints a value to them, and they can't switch to a protected env like `prod` or create your vault. A written env file can still expose values.
- When an agent or script runs `poenv run`, values in the command's output (stdout and stderr) are shown as `***`, as-is and base64, URL-encoded or JSON-escaped. Values shorter than 6 characters aren't masked, and a value the command changes in other ways (reversed, split, hashed) still gets through. A person typing in a terminal sees the output unchanged, even redirected to a file.
- Values are never printed by agent-usable commands, never accepted as arguments, and never in `--json` output.
- A person is detected by an interactive terminal **and** no AI-agent environment markers (`CLAUDECODE`, `CODEX_SANDBOX`, `CURSOR_AGENT`, `GEMINI_CLI`, `AI_AGENT`, …), and no `CI` variable, so CI jobs count as scripts.
- Once a value is written to an env file, anything that can read that file can read the value. `poenv run` avoids writing files at all, and `poenv values run` keeps the env file to key names only, which works for every agent and platform. Only a person can switch a repo back. `.poenv` itself isn't protected, so an agent that sets out to edit it can still undo this.
- `repos.json` next to the vault lists the paths of repos that use poenv, unencrypted (no values, and the repos' `.poenv` files already name their apps).

If `keys.json` is lost, `poenv recover` rebuilds it from your recovery phrase.
