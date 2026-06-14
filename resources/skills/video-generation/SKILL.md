---
name: video-generation
description: Use when the user asks to create or generate a text-to-video clip through the internal media CLI.
allowed-tools:
  - Bash
user-invocable: true
disable-model-invocation: false
requires-action: true
---

Use foreground Bash only. allowed-tools is guidance, not the enforcement boundary; media generation is enforced by the internal tool permission path.
Progress-only or promise-only replies are not completion: after activation, either run `videogen` or report the concrete blocker plainly.

- Run `videogen --prompt ...` for one logical video-generation request.
- Set an explicit long Bash timeout within the current Bash cap before running the command.
- Treat `--prompt` as literal text unless it names a workspace-relative file; prompt file paths should be passed through `--prompt`.
- Pass `--parameters-json` only for requested scalar overrides: strings, numbers, or booleans.
- Pass one `--image-reference` for each public `https://` image URL or approved `asset://` BytePlus asset the user wants to use. Keep the order stable and refer to them in the prompt as image 1, image 2, and so on.
- Do not pass local paths, `file://` URLs, base64 strings, or data URLs as image references. Ask the user to stage or upload the local image first.
- Use `--purpose` only when the request supplies a purpose that should be preserved in result metadata.
- Relaydance is prompt-only in Puffer. If the selected provider is Relaydance and the user asks for image references, report that the configured provider does not support image references.
- If video generation fails or the media runtime is unavailable, report that plainly.
- Do not imply a video was created unless the tool returns a persisted video artifact.
