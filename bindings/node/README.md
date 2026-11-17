# @tactics/wows-render

Node 22 wrapper around the `wows-render` Rust CLI: turn a World of Warships
replay into an MP4. No runtime dependencies; it spawns the binary (no shell)
and speaks its stdin/stdout/stderr contract.

## Install / build

```sh
cd bindings/node
npm install
npm run build   # tsc -> dist/
npm test        # builds, then runs node:test against a fake binary
```

The package is private and ESM-only. Import it from a sibling project with
`import { renderReplay } from "@tactics/wows-render"` (e.g. via `npm link` or a
`file:` dependency). You also need the `wows-render` binary and a data
directory (`defs/<major>_<minor>_<patch>/`, optional `gameparams/`, `maps/`).

## Usage

```ts
import { renderReplay, RenderError, supportedVersions } from "@tactics/wows-render";

const result = await renderReplay({
  input: "/replays/match.wowsreplay",   // or a Buffer / Uint8Array (sent via stdin)
  dataDir: "/srv/wows-data",
  speed: 32,     // default: a 20-minute battle becomes 37.5 s
  maxMb: 8,      // default: the MP4 never exceeds 8 MB
  onProgress: (p) => console.log(p.stage, p.done, p.total),
  signal: AbortSignal.timeout(180_000),
});
// no `output` given -> result.mp4 is a Buffer; with `output` -> result.path is set
```

Options: `input`, `output?`, `dataDir`, `binary?` (a path, or
`[command, ...args]`), `speed?`, `duration?`, `maxMb?`, `fps?`, `width?`,
`height?`, `onProgress?`,
`signal?`, `timeoutMs?` (default 120000; `0` disables).
Result: `{ mp4?, path?, frames, bytes, warnings }`.

`supportedVersions(dataDir)` returns the game versions with definitions, e.g.
`["15_9_0"]`.

### discord.js command handler

```ts
import { AttachmentBuilder, SlashCommandBuilder, type ChatInputCommandInteraction } from "discord.js";
import { renderReplay, RenderError } from "@tactics/wows-render";

export const data = new SlashCommandBuilder()
  .setName("render")
  .setDescription("Render a replay to a video")
  .addAttachmentOption((o) => o.setName("replay").setDescription(".wowsreplay file").setRequired(true));

export async function execute(interaction: ChatInputCommandInteraction) {
  const att = interaction.options.getAttachment("replay", true);
  await interaction.deferReply();

  try {
    const bytes = Buffer.from(await (await fetch(att.url)).arrayBuffer());
    const r = await renderReplay({
      input: bytes,
      dataDir: process.env.WOWS_DATA_DIR!,
      maxMb: 8, // the CLI guarantees the cap, or fails with too_large
    });

    await interaction.editReply({
      content: r.warnings.length ? `Warnings: ${r.warnings.join("; ")}` : undefined,
      files: [new AttachmentBuilder(r.mp4!, { name: "replay.mp4" })],
    });
  } catch (e) {
    if (e instanceof RenderError) {
      console.error(e.code, e.message, e.stderr);
      const msg =
        e.code === "unsupported_version" ? `This replay's game version is not supported: ${e.message}`
        : e.code === "bad_replay" ? "That file does not look like a valid replay."
        : e.code === "timeout" ? "Rendering took too long."
        : e.code === "too_large" ? "This battle does not fit the upload size limit."
        : "Rendering failed.";
      await interaction.editReply(msg);
    } else {
      throw e;
    }
  }
}
```

Interaction tokens expire after 15 minutes, so keep `timeoutMs` well below that.

## Errors

`RenderError` has `code`, `message`, `stderr` (last ~50 non-progress stderr
lines) and `exitCode`.

| code | meaning |
| --- | --- |
| `unsupported_version` | no entity definitions for the replay's game version (message names it) |
| `bad_replay` | replay unreadable or corrupt |
| `missing_data` | data dir / required files missing (also thrown by `supportedVersions` when `defs/` is unreadable) |
| `io` | I/O failure in the CLI, or output file missing after a clean exit |
| `too_large` | the video cannot be made to fit `maxMb` (after one lower-bitrate retry) |
| `internal` | any other CLI failure, including a non-zero exit with no error JSON and unknown CLI codes |
| `timeout` | `timeoutMs` elapsed; child killed (SIGKILL) |
| `aborted` | `signal` aborted; child killed |
| `spawn_failed` | the binary could not be started (not found, not executable) |

## Environment variables

- `WOWS_RENDER_BIN`: path to the `wows-render` binary, used when the `binary`
  option is not given. Otherwise `wows-render` is looked up on `PATH`.

The wrapper itself reads no other variables; the child inherits the parent's
environment.
