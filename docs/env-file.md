# How the env file is written

poenv owns one block at the top of the env file and never touches anything else (except during `poenv init` and `poenv import`, which move the keys they import into the block):

```dotenv
# >>> poenv >>> (managed: edits here are overwritten)
# app: my-app  env: staging
DATABASE_URL=postgres://staging-db/app
OPENAI_API_KEY='sk-…'
# <<< poenv <<<

DEBUG=true   # yours: kept as-is, and it wins over the block
```

A change made anywhere (`set`, `add`, `rm`, `rollback`, or an edit in the TUI) also rewrites the block in every other repo on this machine whose current env it affects, and lists those files by key name. poenv remembers each repo it runs in (paths only, in `repos.json` next to the vault, not in backups) and forgets repos whose `.poenv` is gone. Each repo stays on its own env, and running apps still need a restart unless they reload `.env` themselves.

Key names in the vault use letters, digits and `_` (not starting with a digit). Env files also allow `.` and `-` in names; `poenv init` and `poenv import` leave such keys where they are, outside the block, and say which ones.

Values are quoted for the loader the repo uses (`format` in `.poenv`: `dotenv`, `next` or `python`), because they disagree about `$` and escapes. This is checked against the real Node dotenv, Next.js and python-dotenv loaders in CI.

poenv isn't tied to a framework: anything that reads a `.env`-style file works. `poenv init` picks the file and format from the repo, and `--file` / `--format` (or editing `.poenv`) override them:

| Repo | File | Format |
|---|---|---|
| Next.js (`next` in `package.json`) | `.env.local` | `next` |
| Python (`pyproject.toml`, `requirements.txt` or `Pipfile`) | `.env` | `python` |
| Anything else: Vite, Remix, Express, Bun, Docker Compose… | `.env` | `dotenv` |

If the repo's env files already hold keys, `init` uses the file that has them (and asks when several do). The format only matters for values containing `$`. Tools that expand `$VAR` in env files, like Vite and Bun, read the `dotenv` format's unescaped `$` as a variable, so use `--format next` there, which escapes it as `\$`. Only the three loaders above are tested in CI. `poenv run` sidesteps quoting entirely.

## Keeping values out of the file

Anything that can read the env file can read the values in it, AI agents included, whatever their rules say. To stop that, keep values only inside `poenv run`:

```bash
poenv values run
```

The block then lists key names with empty values, and every later write (`use`, `set`, other repos refreshed after a shared key changes) keeps it that way:

```dotenv
# >>> poenv >>> (managed: edits here are overwritten)
# app: my-app  env: staging
DATABASE_URL=
OPENAI_API_KEY=
# <<< poenv <<<
```

Start anything that reads the env file through `poenv run`, for example `poenv run -- npm run dev`. Node dotenv, Next.js and python-dotenv don't override variables that are already set, so the injected values win over the empty ones. Agents can switch a repo to `run`, but only a person can switch it back with `poenv values file`, since that writes the values where agents can read them.

## Java and Spring Boot

Spring Boot doesn't read `.env` files, so `poenv use` alone changes nothing for it. Inject the env with `poenv run` instead:

```bash
poenv run -- ./mvnw spring-boot:run
poenv run staging -- ./gradlew bootRun
```

Spring reads environment variables natively: reference them in `application.properties` (`spring.datasource.password=${DB_PASSWORD}`), or name keys for relaxed binding (`SPRING_DATASOURCE_URL` sets `spring.datasource.url`). Nothing is written to disk, and no quoting applies.

If you run the app with Docker Compose, `env_file: .env` reads the file poenv writes. Avoid loading it with `spring.config.import=optional:file:.env[.properties]`: properties files don't strip quotes, so any value poenv quotes keeps its quotes.
