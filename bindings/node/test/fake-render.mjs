#!/usr/bin/env node
// Fake `wows-render` used by the tests. The mode is chosen with a leading
// `--mode=<name>` argument (the tests pass it through the `binary` array).
import { readFileSync, writeFileSync } from "node:fs";

const argv = process.argv.slice(2);
let mode = "ok";
const args = [];
for (const a of argv) {
  if (a.startsWith("--mode=")) mode = a.slice("--mode=".length);
  else args.push(a);
}

const input = args[0];
const flag = (name) => {
  const i = args.indexOf(name);
  return i === -1 ? undefined : args[i + 1];
};
const out = flag("--out");
const err = (o) => process.stderr.write(JSON.stringify(o) + "\n");

function fail(code, message, exit = 1) {
  err({ error: { code, message } });
  process.exit(exit);
}

function emit(payload) {
  if (out === "-") {
    process.stdout.write(payload, () => process.exit(0));
  } else {
    writeFileSync(out, payload);
    process.exit(0);
  }
}

const stdinBytes = input === "-" ? readFileSync(0) : Buffer.alloc(0);

// MP4 stand-in: header, stdin bytes, then the JSON of the args received.
function body() {
  return Buffer.concat([
    Buffer.from("FAKEMP4|"),
    stdinBytes,
    Buffer.from("|" + JSON.stringify(args)),
  ]);
}

switch (mode) {
  case "ok": {
    const b = body();
    err({ stage: "done", bytes: b.length, frames: 900, warnings: [] });
    emit(b);
    break;
  }
  case "progress": {
    for (const stage of ["parse", "simulate", "render", "encode"]) {
      for (let d = 0; d <= 2; d++) err({ stage, done: d, total: 2 });
    }
    const b = body();
    err({ stage: "done", bytes: b.length, frames: 60, warnings: ["unknown ship id 42", "no minimap"] });
    emit(b);
    break;
  }
  case "split": {
    // Lines split across writes, CRLF endings, blank lines and plain log noise.
    const b = body();
    const lines =
      'starting up\n{"stage":"parse","done":1,"total":3}\r\n\n' +
      'not json {oops\n{"stage":"render","done":3,"total":3}\n' +
      JSON.stringify({ stage: "done", bytes: b.length, frames: 7, warnings: ["w"] });
    const parts = [];
    for (let i = 0; i < lines.length; i += 7) parts.push(lines.slice(i, i + 7));
    let i = 0;
    const tick = () => {
      if (i < parts.length) {
        process.stderr.write(parts[i++]);
        setTimeout(tick, 1);
      } else {
        process.stderr.write("\n");
        emit(b);
      }
    };
    tick();
    break;
  }
  case "unsupported":
    process.stderr.write("loading defs\n");
    fail("unsupported_version", "no entity definitions for game version 99_9_9");
    break;
  case "bad_replay":
    fail("bad_replay", "truncated replay");
    break;
  case "too_large":
    fail("too_large", "the video would be 9000000 bytes, over the 8000000-byte limit");
    break;
  case "weird_code":
    fail("something_new", "huh");
    break;
  case "crash":
    process.stderr.write("thread 'main' panicked at src/main.rs\n");
    process.exit(101);
    break;
  case "silent_fail":
    process.exit(2);
    break;
  case "hang":
    err({ stage: "parse", done: 0, total: 1 });
    setInterval(() => {}, 1000);
    break;
  case "big": {
    // ~8 MiB output to exercise stdout chunk collection.
    const b = Buffer.alloc(8 * 1024 * 1024, 0xab);
    err({ stage: "done", bytes: b.length, frames: 1, warnings: [] });
    emit(b);
    break;
  }
  case "nodone": {
    emit(body());
    break;
  }
  default:
    fail("internal", `unknown fake mode ${mode}`);
}
