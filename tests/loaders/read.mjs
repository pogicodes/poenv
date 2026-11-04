// Reads <dir>/.env with a real loader and prints the T_* keys as JSON. Used by tests/loaders.rs.
//   node read.mjs dotenv|next|expo <dir>
import { readFileSync } from "node:fs";
import { join } from "node:path";
import dotenv from "dotenv";
import nextEnv from "@next/env";
import expoEnv from "@expo/env";

const [dialect, dir] = process.argv.slice(2);
let got;
if (dialect === "dotenv") {
  got = dotenv.parse(readFileSync(join(dir, ".env")));
} else if (dialect === "expo") {
  got = expoEnv.parseProjectEnv(dir, { mode: "development", silent: true, systemEnv: {} }).env;
} else {
  for (const k of Object.keys(process.env)) if (k.startsWith("T_")) delete process.env[k];
  got = nextEnv.loadEnvConfig(dir, true, { info() {}, error() {} }, true).combinedEnv;
}
console.log(JSON.stringify(Object.fromEntries(Object.entries(got).filter(([k]) => k.startsWith("T_")))));
