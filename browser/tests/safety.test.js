import test from "node:test";
import assert from "node:assert/strict";
import { extract, safeName, uniqueName } from "../public/safety.js";

test("path-like and Windows device names cannot become unsafe download names", () => {
  for (const input of ["../../secret.txt", "C:\\Users\\private.txt", "..\\..\\file.txt", "CON", "con.txt", "LPT¹.txt", ".", "..", "a\u0000b.txt", "<img src=x onerror=alert(1)>"]) {
    const name = safeName(input);
    assert.ok(name.length > 0);
    assert.doesNotMatch(name, /[\\/\x00-\x1f\x7f<>:"|?*]/);
    assert.doesNotMatch(name, /^(con|prn|aux|nul|lpt[0-9¹²³]|com[0-9¹²³])(\.|$)/i);
    assert.doesNotMatch(name, /[. ]$/);
  }
});
test("Unicode remains intact and long names respect the UTF-8 byte bound", () => {
  assert.equal(safeName("مرحبا 🌿.txt"), "مرحبا 🌿.txt");
  const name = safeName("🌿".repeat(300) + ".txt");
  assert.ok(Buffer.byteLength(name) <= 220);
  assert.doesNotMatch(name, /\uFFFD/);
});
test("case-insensitive duplicates receive distinct, predictable names", () => {
  const used = new Set(["same.txt", "same (2).txt"]);
  assert.equal(uniqueName("Same.txt", used), "Same (3).txt");
  assert.equal(uniqueName("empty", new Set(["empty"])), "empty (2)");
});
test("transfer links use only a fragment and reject unrelated or malformed URLs", () => {
  assert.equal(extract(" blobtest "), "blobtest");
  assert.equal(extract("https://akoum.me/dropwire/receive#blobtest"), "blobtest");
  for (const input of ["javascript:blobtest", "https://akoum.me/dropwire/send#blobtest", "https://akoum.me/dropwire/receive?code=blobtest", "https://akoum.me/dropwire/receive#%ZZ", "blob".repeat(10000)]) {
    assert.throws(() => extract(input));
  }
});
