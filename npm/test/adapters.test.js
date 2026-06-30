"use strict";

const assert = require("assert");
const { tareBaseUrl, withTare, tareMiddleware, startProxy } = require("../src/adapters");

// tareBaseUrl — default and custom port
assert.strictEqual(tareBaseUrl(), "http://127.0.0.1:8787", "default port");
assert.strictEqual(tareBaseUrl(9000), "http://127.0.0.1:9000", "custom port");

// withTare — sets both baseURL and base_url
const basic = withTare({});
assert.strictEqual(basic.baseURL, "http://127.0.0.1:8787", "withTare() baseURL default");
assert.strictEqual(basic.base_url, "http://127.0.0.1:8787", "withTare() base_url default");

// withTare — preserves caller-supplied options, custom port
const merged = withTare({ apiKey: "sk-test", timeout: 5000 }, 9001);
assert.strictEqual(merged.apiKey, "sk-test", "withTare preserves apiKey");
assert.strictEqual(merged.timeout, 5000, "withTare preserves timeout");
assert.strictEqual(merged.baseURL, "http://127.0.0.1:9001", "withTare custom port baseURL");
assert.strictEqual(merged.base_url, "http://127.0.0.1:9001", "withTare custom port base_url");

// withTare — original object is not mutated
const orig = { apiKey: "original" };
withTare(orig, 9999);
assert.strictEqual(orig.baseURL, undefined, "withTare does not mutate input");

// tareMiddleware — returns correct LanguageModelV1Middleware shape
const mw = tareMiddleware(8787);
assert.strictEqual(typeof mw.transformParams, "function", "has transformParams");
assert.strictEqual(typeof mw.wrapGenerate, "function", "has wrapGenerate");
assert.strictEqual(typeof mw.wrapStream, "function", "has wrapStream");

// tareMiddleware — transformParams injects _tare_base
mw.transformParams({ params: { model: "claude-3-5-sonnet" } }).then(function (result) {
  assert.strictEqual(result._tare_base, "http://127.0.0.1:8787", "transformParams sets _tare_base");
});

// startProxy — exported as a function (no binary launch in unit test)
assert.strictEqual(typeof startProxy, "function", "startProxy is a function");

console.log("All adapter tests passed.");
