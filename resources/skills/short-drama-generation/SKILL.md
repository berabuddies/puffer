---
name: short-drama-generation
description: Use when the user asks to create a short drama from a prompt — e.g. "生成短剧", "制作微短剧", "make a short drama", "turn this script into a short drama", "ショートドラマを生成", "숏드라마를 만들어". Orchestrates script, storyboard, optional character images, per-shot video clips, and ffmpeg composition through the existing media tools.
allowed-tools:
  - Bash
  - Read
  - Write
user-invocable: true
disable-model-invocation: false
requires-action: true
---

You orchestrate a short drama by driving the existing media tools yourself. There is
no single short-drama-generation tool. allowed-tools is guidance; media generation is enforced by
the internal tool permission path.

Trigger only on a request to CREATE/generate a short drama. Requests to analyze,
rewrite, summarize, or brainstorm a script do NOT trigger this skill unless the user
asks to produce the drama. Progress-only or promise-only replies are not completion:
after starting, either drive the pipeline or report the concrete blocker plainly.

## Pipeline (run in order; skip any stage whose inputs the prompt already supplies)

Pick a short kebab slug `<id>` from the drama title. Put project files under
`.puffer/media/drama/<id>/`. Generated image/video artifacts are written by the tools
to `.puffer/media/images|videos/` — you only reference them, never relocate them.

1. **Script.** If the prompt already contains a script (or names a script file), use it.
   Otherwise write one yourself and save it to `.puffer/media/drama/<id>/script.md`.

2. **Storyboard.** If the prompt already contains a shot breakdown, use it. Otherwise
   break the script into ordered shots (aim for a handful; one beat per shot). Give each
   shot a stable lowercase id (`shot-001`, `shot-002`, …) and record: subject, action,
   scene, lighting, camera, style, target duration (seconds), which characters appear,
   and any stability constraints. These fields become the video prompt — richer shots
   yield better clips. Save to `.puffer/media/drama/<id>/storyboard.md`.

3. **Character images (reference for video).** Scan the prompt for image references that
   are `https://` or `asset://` URLs.
   - If present, use those URLs directly as `--image-reference` in stage 4. Do NOT
     generate images.
   - If absent and the user wants character-consistent shots, generate each character
     once with `imagegen --prompt "<character sheet>" --count 1`. Make the character art
     stylized / non-photorealistic (cartoon, 3D render, illustration): image-to-video
     providers (e.g. BytePlus) reject photoreal real-person images on moderation. Read
     the tool result's `remoteSourceUrl` for that artifact (same key the video tool uses):
       - If `remoteSourceUrl` is present, use it as `--image-reference` in stage 4.
       - If `remoteSourceUrl` is absent, stop and report that the configured image
         provider does not produce a referenceable URL, so image-to-video is unavailable.
         Do NOT silently fall back to text-to-video.
   - If absent and consistency is not required, run text-to-video in stage 4.

4. **Per-shot video.** For each shot in storyboard order, run one `videogen` command:
   - `videogen --prompt "<shot visual + action>"`
   - Add `--image-reference <url>` once per `https://`/`asset://` reference the shot uses;
     keep order stable and refer to them as image 1, image 2, … in the prompt.
   - Each `videogen` call blocks until that clip is finished (the tool polls the provider
     to completion), so set an explicit long Bash timeout within the current Bash cap —
     budget per shot, not for the whole drama. One call → one finished clip.
   - Read `path` from the tool result and record it into the manifest (see below).

5. **Compose.** Stitch the successful shot clips in storyboard order with ffmpeg. First
   probe ffmpeg: `command -v ffmpeg`. If missing, stop and report — do not fake a file.
   Include only shots whose video succeeded; if none succeeded, skip composition and
   report. Build the concat list with single-quote escaping (each clip line is
   `file '<path>'`, with any `'` in the path written as `'\''`). Prefer stream-copy
   (clips from the same provider share codec/params); only if concat-copy fails with a
   codec/params mismatch, retry with a re-encode:

   ```bash
   : > .puffer/media/drama/<id>/concat.txt
   # append one line per SUCCEEDED clip, in order (escape single quotes):
   printf "file '%s'\n" "<clip path, ' -> '\\''>" >> .puffer/media/drama/<id>/concat.txt
   # primary: fast, no re-encode
   ffmpeg -f concat -safe 0 -i .puffer/media/drama/<id>/concat.txt \
     -c copy .puffer/media/drama/<id>/final.mp4
   # fallback only if the copy fails on mismatched streams:
   ffmpeg -f concat -safe 0 -i .puffer/media/drama/<id>/concat.txt \
     -c:v libx264 -pix_fmt yuv420p .puffer/media/drama/<id>/final.mp4
   ```

   If some shots failed but others composed, report it as a partial drama and list the
   missing shot ids.

## Manifest (your working ledger — keep it simple)

Maintain `.puffer/media/drama/<id>/manifest.json` as you go. It is a plain ordered list,
not a schema'd artifact:

```json
{
  "id": "<id>",
  "shots": [
    { "shotId": "shot-001", "status": "succeeded", "prompt": "...", "imageReferences": ["https://..."], "videoArtifactId": "...", "videoPath": ".puffer/media/videos/<aid>/..." }
  ],
  "final": ".puffer/media/drama/<id>/final.mp4"
}
```

## Failure contracts (never paper over)

- If a chosen video provider is Relaydance (prompt-only) and the user wants image
  references, report that the configured provider does not support image references.
- If ffmpeg is unavailable or composition fails, report it plainly and keep the
  per-shot clips; do not claim a composed drama was produced.
- Report final-video success only when `final.mp4` actually exists; a missing final
  video can still leave useful per-shot clips — say so rather than implying success.
- Do not hand-author placeholder media (SVG, stills, stub mp4) and present it as
  generated output.
