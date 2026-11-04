# Commands

| Daily | |
|---|---|
| `poenv use [ENV]` | Switch this repo's env file to ENV (picker if omitted) |
| `poenv add KEY [--as NAME] [--from APP/ENV] [--env ENV]` | Add a shared key (live reference) or copy one from another app |
| `poenv status [--short]` | Current app/env (`--short` → `my-app:staging` for your prompt) |

| Setup & management | |
|---|---|
| `poenv init [--app NAME] [--env ENV] [--file FILE] [--format F] [--no-import] [--yes]` | Set up a repo: import the env file, write `.poenv`, gitignore, agent notes |
| `poenv set KEY [--shared] [--env ENV] [--empty]` | Set a value (asked for hidden, or piped in; never as an argument) |
| `poenv import [--env ENV]` | Save keys you pasted into the env file to the current env (or ENV), overwriting existing ones, and move them into the block |
| `poenv rm KEY [--shared [--keep-copies \| --everywhere]] [--env ENV]` | Remove a key. A shared key that apps still use needs a choice: `--keep-copies` (each app keeps its own copy, which undoes sharing) or `--everywhere` |
| `poenv env new NAME [--from ENV]` | Add an env with any name (empty, or a copy of ENV) |
| `poenv env rename OLD NEW` | Rename an env; the env file and `.poenv` follow |
| `poenv file [NAME]` | Show or change the env file poenv manages (e.g. `.env`); the block moves with it |
| `poenv values [file\|run]` | Keep values in the env file (default), or only inside `poenv run` so the file lists key names only. Back to `file` is human-only |
| `poenv ls [APP]` | Apps, envs and key names |
| `poenv run [ENV] -- CMD` | Run CMD with ENV injected, without touching the file (agents see values in its output as `***`) |
| `poenv doctor [--fix]` | Check and fix your setup |

| Just for you (refused for agents and scripts) | |
|---|---|
| `poenv get KEY [--shared] [--env ENV]` | Print a value |
| `poenv history KEY [--show]` / `poenv rollback KEY [N]` | Previous values (last 10, masked unless `--show`) / restore one |
| `poenv backup [FILE] [--force]` / `poenv restore FILE` | Encrypted backup / restore |
| `poenv recover [--check \| --new-phrase]` | Rebuild this machine's key from your phrase / check your saved phrase / make a new phrase |

`history`, `rollback` and `rm` also take `--shared` / `--env ENV`. Every command takes `--json` for machine-readable output (except `poenv run`, which passes through CMD's output and exit code) and uses stable exit codes: `0` ok · `1` error · `2` not set up · `3` unknown app/env/key · `4` refused (human-only or protected) · `5` waiting on the user.

## The TUI

Run `poenv` on its own in a terminal to browse and edit everything: apps, environments and keys, with values masked until you press `v`. Inside a repo it opens on that repo's app, and `u` switches the env file to the selected env. On another app, `u` switches that app's repo on this machine (asking which one if there are several), so you can switch any repo from anywhere. `poenv use` on the command line only ever switches the folder you're in. Lowercase keys act on the selected key, uppercase on the whole env (`A` new, `R` rename, `P` protect, `D` delete). Protecting or unprotecting an env and deleting a whole env are only in the TUI: there's no command for them.
