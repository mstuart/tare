const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const { verifyChecksum } = require("../install");

const payload = Buffer.from("verified release payload");
const digest = crypto.createHash("sha256").update(payload).digest("hex");

assert.doesNotThrow(() => verifyChecksum(payload, `${digest}  tare.tar.gz\n`));
assert.throws(
  () => verifyChecksum(payload, `${"0".repeat(64)}  tare.tar.gz\n`),
  /checksum mismatch/
);
assert.throws(() => verifyChecksum(payload, ""), /invalid SHA-256 checksum/);
assert.throws(
  () => verifyChecksum(payload, "not-a-checksum  tare.tar.gz\n"),
  /invalid SHA-256 checksum/
);

console.log("All installer tests passed.");
