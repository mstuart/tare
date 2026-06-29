#!/usr/bin/env node
"use strict";
const { spawnSync } = require("child_process");
const path = require("path");
const bin = path.join(__dirname, "..", "vendor", "tare-mcp" + (process.platform === "win32" ? ".exe" : ""));
const r = spawnSync(bin, process.argv.slice(2), { stdio: "inherit" });
if (r.error) {
  console.error(
    r.error.code === "ENOENT"
      ? "[tare] binary missing — reinstall: npm install -g @mstuart/tare"
      : "[tare] " + r.error.message
  );
  process.exit(1);
}
process.exit(r.status === null ? 1 : r.status);
