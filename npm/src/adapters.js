"use strict";

const path = require("path");
const { spawn } = require("child_process");

/**
 * Returns the tare proxy base URL for a given port.
 * @param {number} [port=8787]
 * @returns {string}
 */
function tareBaseUrl(port) {
  if (port === undefined) port = 8787;
  return "http://127.0.0.1:" + port;
}

/**
 * Merges the tare proxy base URL into SDK client options.
 * Sets both `baseURL` (Anthropic/OpenAI JS SDK) and `base_url` (some older clients).
 * Use this to construct an SDK client that routes through the tare proxy:
 *
 *   const anthropic = new Anthropic(withTare({ apiKey: process.env.ANTHROPIC_API_KEY }));
 *   const openai    = new OpenAI(withTare({ apiKey: process.env.OPENAI_API_KEY }));
 *
 * @param {object} [clientOptions={}]
 * @param {number} [port=8787]
 * @returns {object}
 */
function withTare(clientOptions, port) {
  if (clientOptions === undefined) clientOptions = {};
  if (port === undefined) port = 8787;
  const base = tareBaseUrl(port);
  return Object.assign({}, clientOptions, {
    baseURL: base,
    base_url: base,
  });
}

/**
 * Returns a minimal LanguageModelV1Middleware-shaped object (Vercel AI SDK compatible)
 * whose transformParams attaches the proxy base URL as `_tare_base`.
 *
 * NOTE: actual HTTP routing requires the underlying provider client to be constructed
 * with withTare() — this middleware alone does not intercept fetch calls. It is a
 * structural pass-through that carries the proxy URL into the call chain so custom
 * providers / fetch wrappers can honour it.
 *
 * No hard dependency on the Vercel AI SDK package — typed loosely for drop-in use.
 *
 * @param {number} [port=8787]
 * @returns {{ transformParams: Function, wrapGenerate: Function, wrapStream: Function }}
 */
function tareMiddleware(port) {
  if (port === undefined) port = 8787;
  const base = tareBaseUrl(port);
  return {
    transformParams: async function (opts) {
      return Object.assign({}, opts.params, { _tare_base: base });
    },
    wrapGenerate: async function (opts) {
      return opts.doGenerate();
    },
    wrapStream: async function (opts) {
      return opts.doStream();
    },
  };
}

/**
 * Spawns the vendored tare-proxy binary and returns a handle.
 *
 *   const { baseUrl, stop } = startProxy({ port: 8787 });
 *   const anthropic = new Anthropic({ baseURL: baseUrl, apiKey: "..." });
 *   // ...
 *   stop();
 *
 * @param {{ port?: number, args?: string[] }} [opts={}]
 * @returns {{ child: import('child_process').ChildProcess, baseUrl: string, stop: function(): void }}
 */
function startProxy(opts) {
  if (opts === undefined) opts = {};
  const port = opts.port !== undefined ? opts.port : 8787;
  const extraArgs = opts.args || [];
  const ext = process.platform === "win32" ? ".exe" : "";
  const bin = path.join(__dirname, "..", "vendor", "tare-proxy" + ext);

  const child = spawn(bin, ["--port", String(port)].concat(extraArgs), {
    stdio: "inherit",
  });

  child.on("error", function (err) {
    console.error(
      err.code === "ENOENT"
        ? "[tare] tare-proxy binary missing — reinstall: npm install -g tare-ai"
        : "[tare] " + err.message
    );
  });

  return {
    child: child,
    baseUrl: tareBaseUrl(port),
    stop: function () {
      child.kill();
    },
  };
}

module.exports = { tareBaseUrl, withTare, tareMiddleware, startProxy };
