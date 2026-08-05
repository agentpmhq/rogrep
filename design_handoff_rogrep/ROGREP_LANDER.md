# rogrep lander — change spec

Replaces what is live at https://agentpmhq.github.io/rogrep/

Design reference: `Rogrep Page.dc.html` (open in a browser). Content is **verbatim** from the deployed page except where this file says otherwise — the changes are structural and to signal, not to claims.

## Why

The page is substantively open — Apache-2.0 stated three times, install first, no account, real links to CONTRIBUTING and LICENSE. But it reads as a commercial subpage, because structurally it is one: `apps/ironscribe/app/layout.tsx` wraps it in `CompanyNav` whose five items are `rogrep · agentpm · services · blog · company`, four of which leave for agentpm.dev. A developer's first three seconds are spent on a SaaS nav.

Five conventional open-source markers are missing. Adding them is most of the fix.

---

## Task 1 — a rogrep shell, not the company shell

**This is the change that matters.** Everything else is decoration on top of it.

The GitHub Pages export currently inherits `apps/ironscribe/app/layout.tsx`. It needs its own header and footer.

Add to `packages/brand`:

**`RogrepNav`** — sticky, `1px solid var(--is-ink)` bottom border, on `var(--is-surface)`:
- Left: `rogrep` in mono 19px/700, `letter-spacing:-0.02em`, with the version beside it in mono 11px `var(--is-ink-4)`.
- Centre: `docs · benchmarks · discussions · contributing` — mono 12.5px, `var(--is-ink-2)`. Every one points at the repo or an on-page anchor. **None point at agentpm.dev.**
- Right: a **star button** — bordered `1px solid var(--is-ink)`, GitHub mark + `Star` on `var(--is-surface)`, then the count in a `var(--is-ink)` block with `var(--is-surface)` text. Two cells divided by a 1px rule, no radius.
- Then the `install` CTA in `var(--is-accent)`.
- Under 720px: collapse the centre nav to a disclosure panel; the star button and install CTA stay visible.

**`RogrepFooter`** — three groups: the `rogrep` wordmark + one-line description + Apache-2.0; **Project** (repository, documentation, issues, Apache-2.0 — all external); **Iron Scribe** (ironscribe.co, agentpm, services, hello@ironscribe.co). Attribution kept, demoted to a footer column.

**Company strip** — one thin `var(--is-ink)` bar *above* the nav, mono 10.5px uppercase `letter-spacing:0.16em`: `An Iron Scribe project` left, `Apache-2.0` and `github ↗` right. This is where company attribution belongs on a project page — present, not framing.

How to wire it depends on how the Pages export is configured, which I could not determine from the branch. Either a `(rogrep)` route group with its own `layout.tsx`, or a separate app with its own root layout. Pick whichever matches the existing deploy and say which you chose. The requirement is only that the rogrep lander does not render `CompanyNav`.

## Task 2 — install: publish first, build from source second

Currently the only install path is:

```
git clone https://github.com/agentpmhq/rogrep
cd rogrep
cargo install --path crates/rogrep
```

Build-from-source as the *only* option signals "not published yet." This is the second-biggest perception change on the page.

Replace the single `InstallPanel` with three stacked rows in one bordered group, hairline-divided, in this order:

| Row | Label | Note | Command |
|---|---|---|---|
| 1 | `cargo` | Rust 1.85+ | `cargo install rogrep` |
| 2 | `homebrew` | macOS · Linux | `brew install agentpmhq/tap/rogrep` |
| 3 | `from source` | builds to ~/.local/bin | the existing three-line clone |

**Verify before shipping:** if `rogrep` is not published to crates.io and the tap does not exist, do not show those rows. Promote `from source` to first and delete the others. A copy-paste command that fails is far worse than an honest source build. Say which of the three are real when you report back.

The hero also gets the lead install command inline, in a dark well with a blinking cursor — one line, immediately runnable.

Keep verbatim: "Nothing runs as root, no daemon is installed, and no account is created." Then the three `sync` / `doctor` / `tui` commands as three cells.

## Task 3 — repo facts strip

Five cells under the hero, hairline-divided on `var(--is-surface-raise)`, each a mono uppercase label over a mono 13px value:

`License: Apache-2.0` · `Language: Rust` · `Version: <version>` · `Platforms: macOS · Linux` · `Account: none`

`Account: none` is the one that earns its place — the page already claims it in prose, and a facts row is where a developer looks for it.

## Task 4 — "Built in the open" replaces "rogrep resources"

Same four resource cards (documentation, contributing, discussions, architecture) — drop the separate **License** card, since Apache-2.0 now appears in the company strip, the facts strip, the section intro and the footer. Five mentions was already three too many; six would be comic.

Above the cards, an **activity strip**: stars · contributors · commits · last commit, as mono values over uppercase labels.

**Render only the cells you have real values for, and hide the strip entirely when there are none.** Four em-dashes side by side read as a failed data fetch, which destroys exactly the credibility the strip exists to build.

Section intro: "Apache-2.0, and staying that way. Development happens in public: issues, discussions and review all live on the repository."

## Task 5 — wiring the live figures

Stars, contributors, commits and last-commit are the only new data on the page. **Do not hard-code them.**

The export is static, so fetch at build time in the page component:

```ts
const res = await fetch("https://api.github.com/repos/agentpmhq/rogrep", {
  headers: { Accept: "application/vnd.github+json" },
});
```

- `stargazers_count` → stars (format `1.2k` above 1000), `pushed_at` → last commit as a relative string.
- Contributors: `/contributors?per_page=100` and count. Commits: the `Link` header's last page from `/commits?per_page=1`.
- Unauthenticated is 60 req/hr per IP — fine for a build, but use `GITHUB_TOKEN` in the Actions workflow to be safe.
- **Wrap every call in try/catch and fall back to omitting the value.** A rate-limited API must never fail the build or ship a zero.
- Figures go stale between deploys. Add a `schedule:` trigger to the Pages workflow (weekly is plenty) so they refresh without a code change.

## Task 6 — remove the placeholder stats table

Section 04 currently ships this to production:

> **PLACEHOLDER VALUES — replace with a real paste.**

Delete `statsRows` and the table. Keep the `rogrep stats top --by tokens --since 7d` command and the sentence about SQLite determinism, with a quiet `// illustrative shape — replace with a real paste` marker in `var(--is-ink-4)`.

Better still: paste a real `rogrep stats` run and delete the marker. Redact project names if needed — that it is real matters more than which projects appear.

---

## Deliberately unchanged

- **The cass benchmark stays.** Publishing a competitor benchmark is not a corporate move; ripgrep built its reputation on exactly that. The numbers are real, the caveats are stated, and the methodology is linked. Do not soften it.
- Sections 02–05 keep their copy verbatim.
- The AgentPM bridge band stays, at the bottom, one link, no signup.
- Radius 0, two fonts, container queries, terminal wells as the only dark surfaces.

## Deploying

The site at agentpmhq.github.io/rogrep is a GitHub Pages publish of a static export, so "deploy" means: land the code, build the export, push the artifact. Nothing here reaches production until that build runs.

Before pushing, verify the export — not the dev server. A static export drops anything that needs a server, so a page that looks right at `localhost:3045` can still ship broken:

1. **The build-time GitHub fetch actually ran.** Grep the built HTML for the star count. If it's absent, the fetch failed and the strip correctly hid itself — but you want to know that before it's live, not after.
2. **`basePath` is right.** Pages serves from `/rogrep/`, so relative asset paths resolve differently than at the dev root. Fonts and the GitHub mark are the usual casualties.
3. **No `CompanyNav` in the output.** Grep the built HTML for `services` and `company` as nav items. If they appear, Task 1's layout separation didn't take effect in the export.
4. **Install commands are copy-pasteable.** Copy each one out of the built page and run it. This is where the `--` → `/-` bug shows itself.

Then add the weekly `schedule:` trigger from Task 5 so the activity figures refresh without a code change.

Push to a preview or a branch deploy first if the workflow supports it. If it only publishes from the default branch, that's fine — but then get eyes on it immediately after the first publish rather than assuming.

## Traps

- Terminal wells: **one `<div>` per line**, and blank spacer lines need `min-height:1.8em` or they collapse to zero height. The existing `styles.terminalSpacer` already handles this — keep using it.
- Every SVG `<path>` needs `fill="none"`.
- No color literals. Every value resolves to a `--is-*` token in `packages/brand/tokens.css`.
