const { spawn } = require("node:child_process");
const path = require("node:path");

/**
 * Returns the tare proxy base URL for a given port.
 * @param {number} [port=8787]
 * @returns {string}
 */
function tareBaseUrl(port = 8787) {
  return `http://127.0.0.1:${port}`;
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
function withTare(clientOptions = {}, port = 8787) {
  const base = tareBaseUrl(port);
  return { ...clientOptions, base_url: base, baseURL: base };
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
function tareMiddleware(port = 8787) {
  const base = tareBaseUrl(port);
  return {
    transformParams(opts) {
      return Promise.resolve({ ...opts.params, _tare_base: base });
    },
    wrapGenerate(opts) {
      return opts.doGenerate();
    },
    wrapStream(opts) {
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
function startProxy(opts = {}) {
  const port = opts.port === undefined ? 8787 : opts.port;
  const extraArgs = opts.args || [];
  const ext = process.platform === "win32" ? ".exe" : "";
  const bin = path.join(
    path.dirname(require.resolve("../package.json")),
    "vendor",
    `tare-proxy${ext}`
  );

  const child = spawn(bin, ["--port", String(port)].concat(extraArgs), {
    stdio: "inherit",
  });

  child.on("error", (err) => {
    console.error(
      err.code === "ENOENT"
        ? "[tare] tare-proxy binary missing — reinstall: npm install -g tare-ai"
        : `[tare] ${err.message}`
    );
  });

  return {
    baseUrl: tareBaseUrl(port),
    child,
    stop() {
      child.kill();
    },
  };
}

module.exports = { startProxy, tareBaseUrl, tareMiddleware, withTare };
