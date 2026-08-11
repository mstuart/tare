#!/usr/bin/env node

const { spawnSync } = require("node:child_process");
const path = require("node:path");
const bin = path.join(
  path.dirname(require.resolve("../package.json")),
  "vendor",
  `tare-proxy${process.platform === "win32" ? ".exe" : ""}`
);
const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (r.error) {
  console.error(
    r.error.code === "ENOENT"
      ? "[tare] binary missing — reinstall: npm install -g tare-ai"
      : `[tare] ${r.error.message}`
  );
  process.exit(1);
}
process.exit(r.status === null ? 1 : r.status);
