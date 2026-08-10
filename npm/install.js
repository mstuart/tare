#!/usr/bin/env node

// postinstall: download the prebuilt tare binaries for this platform from the GitHub Release that
// matches this package's version, verify the checksum, and extract them into vendor/. The published
// npm package carries only the JS shims + this script; the (self-contained) binaries are fetched here.
const { execFileSync } = require("node:child_process");
const crypto = require("node:crypto");
const fs = require("node:fs");
const https = require("node:https");
const path = require("node:path");

const REPO = "mstuart/tare";
const pkg = require("./package.json");
const VERSION = process.env.TARE_VERSION || `v${pkg.version}`;
const BASE =
  process.env.TARE_DOWNLOAD_BASE ||
  `https://github.com/${REPO}/releases/download/${VERSION}`;
const WHITESPACE_PATTERN = /\s+/;

function target() {
  const arch = { arm64: "aarch64", x64: "x86_64" }[process.arch];
  const os = { darwin: "apple-darwin", linux: "unknown-linux-gnu" }[
    process.platform
  ];
  return arch && os ? `${arch}-${os}` : null;
}

function fetch(url, redirects = 0) {
  return new Promise((resolve, reject) => {
    if (url.startsWith("file:")) {
      try {
        return resolve(fs.readFileSync(new URL(url)));
      } catch (e) {
        return reject(e);
      }
    }
    https
      .get(url, (res) => {
        if (
          res.statusCode >= 300 &&
          res.statusCode < 400 &&
          res.headers.location
        ) {
          res.resume();
          if (redirects > 10) {
            return reject(new Error("too many redirects"));
          }
          return resolve(fetch(res.headers.location, redirects + 1));
        }
        if (res.statusCode !== 200) {
          res.resume();
          return reject(new Error(`HTTP ${res.statusCode} for ${url}`));
        }
        const chunks = [];
        res.on("data", (c) => chunks.push(c));
        res.on("end", () => resolve(Buffer.concat(chunks)));
      })
      .on("error", reject);
  });
}

async function main() {
  const t = target();
  if (!t) {
    console.error(
      `[tare] no prebuilt binary for ${process.platform}/${process.arch}; ` +
        `build from source: https://github.com/${REPO}`
    );
    return; // soft-fail: don't break the whole npm install
  }
  const asset = `tare-${t}.tar.gz`;
  const vendor = path.join(
    path.dirname(require.resolve("./package.json")),
    "vendor"
  );
  fs.mkdirSync(vendor, { recursive: true });
  const tgz = path.join(vendor, asset);

  console.log(`[tare] downloading ${asset} (${VERSION})…`);
  const buf = await fetch(`${BASE}/${asset}`);
  fs.writeFileSync(tgz, buf);

  // Verify the checksum if one is published alongside the asset.
  try {
    const [sum] = (await fetch(`${BASE}/${asset}.sha256`))
      .toString("utf8")
      .trim()
      .split(WHITESPACE_PATTERN);
    const got = crypto.createHash("sha256").update(buf).digest("hex");
    if (sum && sum !== got) {
      console.error(`[tare] checksum mismatch: ${got} != ${sum}`);
      process.exit(1);
    }
  } catch {
    /* no checksum published — proceed */
  }

  execFileSync("tar", ["-xzf", tgz, "-C", vendor]);
  fs.unlinkSync(tgz);
  for (const b of ["tare", "tare-proxy", "tare-mcp"]) {
    const p = path.join(vendor, b);
    if (fs.existsSync(p)) {
      fs.chmodSync(p, 0o755);
    }
  }
  console.log(`[tare] installed tare, tare-proxy, tare-mcp into ${vendor}`);
}

main().catch((e) => {
  console.error(`[tare] install failed: ${e.message}`);
  process.exit(1);
});
