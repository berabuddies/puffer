---
name: image-generation
description: Use when the user asks to create, generate, or render images through the internal media CLI.
allowed-tools:
  - Bash
user-invocable: true
disable-model-invocation: false
requires-action: true
---

Use foreground Bash only. allowed-tools is guidance, not the enforcement boundary; media generation is enforced by the internal tool permission path.
Progress-only or promise-only replies are not completion: after activation, either run `imagegen` or report the concrete blocker plainly.

- Run one `imagegen --prompt ... --count ...` command for one logical request.
- Set an explicit long Bash timeout within the current Bash cap before running the command.
- When the user asks for multiple images from one prompt, pass the requested number through `--count` instead of issuing repeated `--count 1` commands.
- Treat `--prompt` as literal text unless it names a workspace-relative file; prompt file paths should be passed through `--prompt`.
- Use `--prompt-reference` only when the request supplies additional prompt context.
- Use `--aspect`, `--purpose`, or `--retry-from-error-json` only when the request or retry context requires them.
- If image generation fails or the media runtime is unavailable, report that plainly.
- Do not hand-author SVG, ASCII art, placeholder files, or other substitutes and present them as generated images.
