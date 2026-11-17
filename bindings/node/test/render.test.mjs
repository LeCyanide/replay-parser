import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, mkdir, readFile, rm, symlink, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

import { renderReplay, supportedVersions, RenderError } from "../dist/index.js";

const here = dirname(fileURLToPath(import.meta.url));
const FAKE = join(here, "fake-render.mjs");
const bin = (mode) => [process.execPath, FAKE, `--mode=${mode}`];
const base = { dataDir: "/data", timeoutMs: 20_000 };

async function tmp() {
  return mkdtemp(join(tmpdir(), "wows-render-test-"));
}

function receivedArgs(buf) {
  const s = buf.toString("latin1");
  return JSON.parse(s.slice(s.lastIndexOf("|") + 1));
}

test("file input -> stdout returns an mp4 Buffer and passes the CLI contract args", async () => {
  const r = await renderReplay({
    ...base,
    input: "/replays/a.wowsreplay",
    binary: bin("ok"),
    duration: 12,
    fps: 24,
    width: 640,
    height: 360,
  });
  assert.ok(Buffer.isBuffer(r.mp4));
  assert.equal(r.path, undefined);
  assert.equal(r.frames, 900);
  assert.equal(r.bytes, r.mp4.length);
  assert.deepEqual(r.warnings, []);
  assert.deepEqual(receivedArgs(r.mp4), [
    "/replays/a.wowsreplay", "--out", "-", "--data", "/data",
    "--duration", "12", "--fps", "24", "--width", "640", "--height", "360",
    "--progress", "json",
  ]);
});

test("optional flags are omitted when unset", async () => {
  const r = await renderReplay({ ...base, input: "x.wowsreplay", binary: bin("ok") });
  assert.deepEqual(receivedArgs(r.mp4), [
    "x.wowsreplay", "--out", "-", "--data", "/data", "--progress", "json",
  ]);
});

test("output path writes a file and returns path/bytes/frames", async () => {
  const dir = await tmp();
  try {
    const output = join(dir, "out.mp4");
    const r = await renderReplay({ ...base, input: "a.wowsreplay", output, binary: bin("ok") });
    assert.equal(r.path, output);
    assert.equal(r.mp4, undefined);
    assert.equal(r.frames, 900);
    const file = await readFile(output);
    assert.equal(r.bytes, file.length);
    assert.equal(receivedArgs(file)[2], output);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("without a done line, bytes come from the file size", async () => {
  const dir = await tmp();
  try {
    const output = join(dir, "o.mp4");
    const r = await renderReplay({ ...base, input: "a", output, binary: bin("nodone") });
    assert.equal(r.bytes, (await readFile(output)).length);
    assert.equal(r.frames, 0);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("Buffer input is streamed over stdin", async () => {
  const replay = Buffer.from("replay-bytes-é-\x00\x01");
  const r = await renderReplay({ ...base, input: replay, binary: bin("ok") });
  assert.equal(receivedArgs(r.mp4)[0], "-");
  assert.ok(r.mp4.subarray(8, 8 + replay.length).equals(replay));
});

test("Uint8Array view input (with byteOffset) and large stdin both work", async () => {
  const backing = new Uint8Array(1_000_000 + 10).map((_, i) => i % 251);
  const view = backing.subarray(5, 5 + 1_000_000);
  const r = await renderReplay({ ...base, input: view, binary: bin("ok") });
  assert.ok(r.mp4.subarray(8, 8 + view.length).equals(Buffer.from(view)));
});

test("stdin + file output together", async () => {
  const dir = await tmp();
  try {
    const output = join(dir, "o.mp4");
    const r = await renderReplay({ ...base, input: Buffer.from("zzz"), output, binary: bin("ok") });
    const file = await readFile(output);
    assert.equal(file.subarray(8, 11).toString(), "zzz");
    assert.equal(r.bytes, file.length);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("large stdout is collected completely", async () => {
  const r = await renderReplay({ ...base, input: "a", binary: bin("big") });
  assert.equal(r.mp4.length, 8 * 1024 * 1024);
  assert.equal(r.bytes, r.mp4.length);
});

test("progress events and warnings are delivered", async () => {
  const events = [];
  const r = await renderReplay({
    ...base,
    input: "a",
    binary: bin("progress"),
    onProgress: (p) => events.push(p),
  });
  assert.equal(events.length, 12);
  assert.deepEqual(events[0], { stage: "parse", done: 0, total: 2 });
  assert.deepEqual(events.at(-1), { stage: "encode", done: 2, total: 2 });
  assert.deepEqual(events.map((e) => e.stage).filter((s, i, a) => a.indexOf(s) === i), [
    "parse", "simulate", "render", "encode",
  ]);
  assert.deepEqual(r.warnings, ["unknown ship id 42", "no minimap"]);
  assert.equal(r.frames, 60);
});

test("stderr lines split across chunks, CRLF and noise are parsed robustly", async () => {
  const events = [];
  const r = await renderReplay({
    ...base,
    input: "a",
    binary: bin("split"),
    onProgress: (p) => events.push(p),
  });
  assert.deepEqual(events, [
    { stage: "parse", done: 1, total: 3 },
    { stage: "render", done: 3, total: 3 },
  ]);
  assert.equal(r.frames, 7);
  assert.deepEqual(r.warnings, ["w"]);
});

test("a throwing onProgress does not break the render", async () => {
  const r = await renderReplay({
    ...base,
    input: "a",
    binary: bin("progress"),
    onProgress: () => {
      throw new Error("boom");
    },
  });
  assert.equal(r.frames, 60);
});

test("CLI error JSON becomes a typed RenderError", async () => {
  await assert.rejects(
    renderReplay({ ...base, input: "a", binary: bin("unsupported") }),
    (e) => {
      assert.ok(e instanceof RenderError);
      assert.equal(e.code, "unsupported_version");
      assert.match(e.message, /99_9_9/);
      assert.match(e.stderr, /loading defs/); // plain log line kept
      assert.match(e.stderr, /unsupported_version/);
      assert.equal(e.exitCode, 1);
      return true;
    },
  );
  await assert.rejects(renderReplay({ ...base, input: "a", binary: bin("bad_replay") }), {
    code: "bad_replay",
  });
});

test("unknown CLI error codes map to internal", async () => {
  await assert.rejects(renderReplay({ ...base, input: "a", binary: bin("weird_code") }), {
    code: "internal",
    message: "huh",
  });
});

test("non-zero exit without error JSON -> internal with stderr tail", async () => {
  await assert.rejects(renderReplay({ ...base, input: "a", binary: bin("crash") }), (e) => {
    assert.equal(e.code, "internal");
    assert.equal(e.exitCode, 101);
    assert.match(e.stderr, /panicked/);
    return true;
  });
  await assert.rejects(renderReplay({ ...base, input: "a", binary: bin("silent_fail") }), {
    code: "internal",
  });
});

test("timeout kills the child and rejects with timeout", async () => {
  const t0 = Date.now();
  await assert.rejects(
    renderReplay({ ...base, input: "a", binary: bin("hang"), timeoutMs: 300 }),
    { code: "timeout" },
  );
  assert.ok(Date.now() - t0 < 5000);
});

test("abort during run kills the child and rejects with aborted", async () => {
  const ac = new AbortController();
  const p = renderReplay({ ...base, input: "a", binary: bin("hang"), signal: ac.signal });
  setTimeout(() => ac.abort(), 150);
  await assert.rejects(p, { code: "aborted" });
});

test("already-aborted signal rejects without spawning", async () => {
  const ac = new AbortController();
  ac.abort();
  await assert.rejects(
    renderReplay({ ...base, input: "a", binary: ["/definitely/not/here"], signal: ac.signal }),
    { code: "aborted" },
  );
});

test("missing binary -> spawn_failed", async () => {
  await assert.rejects(
    renderReplay({ ...base, input: "a", binary: "/definitely/not/here/wows-render" }),
    (e) => e instanceof RenderError && e.code === "spawn_failed",
  );
});

test("binary defaults to WOWS_RENDER_BIN", async () => {
  const prev = process.env.WOWS_RENDER_BIN;
  process.env.WOWS_RENDER_BIN = "/definitely/not/here/from-env";
  try {
    await assert.rejects(renderReplay({ ...base, input: "a" }), (e) => {
      assert.equal(e.code, "spawn_failed");
      assert.match(e.message, /from-env/);
      return true;
    });
  } finally {
    if (prev === undefined) delete process.env.WOWS_RENDER_BIN;
    else process.env.WOWS_RENDER_BIN = prev;
  }
});

test("empty binary array -> spawn_failed", async () => {
  await assert.rejects(renderReplay({ ...base, input: "a", binary: [] }), {
    code: "spawn_failed",
  });
});

test("supportedVersions lists usable defs/ subdirectories, sorted numerically", async () => {
  const dir = await tmp();
  try {
    for (const v of ["15_10_0", "15_9_0", "14_11_1"]) {
      await mkdir(join(dir, "defs", v), { recursive: true });
      await writeFile(join(dir, "defs", v, "entities.xml"), "<root/>");
    }
    // A build-number key that is a symlink to a version directory counts.
    await symlink(join(dir, "defs", "15_9_0"), join(dir, "defs", "13533378"));
    // No entities.xml: the renderer would not use it either.
    await mkdir(join(dir, "defs", "15_11_0"));
    await writeFile(join(dir, "defs", "README.txt"), "x");
    await mkdir(join(dir, "maps"));
    const want = ["14_11_1", "15_9_0", "15_10_0", "13533378"];
    assert.deepEqual(await supportedVersions(dir), want);
    assert.deepEqual(await supportedVersions(dir + "/"), want);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("supportedVersions without defs/ -> missing_data", async () => {
  const dir = await tmp();
  try {
    await assert.rejects(supportedVersions(dir), { code: "missing_data" });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
