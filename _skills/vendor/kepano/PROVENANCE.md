# Vendored skill bundle: kepano/obsidian-skills

These skills are vendored (copied verbatim, hash-pinned) from the open-source
**kepano/obsidian-skills** repository.

- **Source:** https://github.com/kepano/obsidian-skills
- **Pinned commit:** `a1dc48e68138490d522c04cbf5822214c6eb1202`
- **License:** MIT — © 2026 Steph Ango (@kepano). Full text in [`LICENSE`](LICENSE).
- **Integrity:** every file's SHA-256 is pinned in [`vendor.lock`](vendor.lock); the
  loader rejects any vendored skill whose `SKILL.md` does not match its pinned hash
  (tamper → not loaded), and any vendored skill without a manifest `license` entry.

## Included skills (vault-native; declared capabilities ⊆ {VaultRead, VaultWrite})

- **obsidian-markdown** — create/edit Obsidian Flavored Markdown (wikilinks, embeds,
  callouts, properties). Ships `references/{PROPERTIES,EMBEDS,CALLOUTS}.md` (tier-3 resources).
- **json-canvas** — read/write the JSON Canvas (`.canvas`) open format. Ships `references/EXAMPLES.md`.

Shell/CLI- and web-shaped kepano skills (`obsidian-cli`, `defuddle`) are intentionally
NOT vendored: their capabilities are inert in this phase (no shell/egress actuator exists),
so bundling them would only present instructions the agent cannot act on.

## Updating the pin

Re-fetch the desired files at a new commit, replace them verbatim, and regenerate
`vendor.lock` (bundle-level: `{bundle, source, commit, license, files:[{rel_path, sha256}]}`).
Keep the upstream `LICENSE` alongside the copies.
