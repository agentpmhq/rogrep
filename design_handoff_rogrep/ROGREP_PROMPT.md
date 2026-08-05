# Prompt for the coding agent

Commit `design_handoff_rogrep/` to the branch first, then run these as **two separate sessions**.

---

## Session A — the shell

> Read `design_handoff_rogrep/ROGREP_LANDER.md` Task 1 and open `design_handoff_rogrep/Rogrep Page.dc.html` in a browser.
>
> The rogrep lander at agentpmhq.github.io/rogrep currently renders inside `apps/ironscribe/app/layout.tsx`, so its nav is `CompanyNav` with five Iron Scribe items, four of which leave for agentpm.dev. Give the lander its own shell: build `RogrepNav` and `RogrepFooter` in `packages/brand` per Task 1, plus the thin Iron Scribe company strip above the nav.
>
> First tell me how the Pages export is configured — route group, separate app, or something else — and which approach you'll use to give it a different root layout. Don't start until we agree on that.
>
> Do not modify `CompanyNav` or the Iron Scribe site's layout. `/ironscribe` and `/agentpm` must be untouched.

## Session B — the page

> Read `design_handoff_rogrep/ROGREP_LANDER.md` Tasks 2–6 and match the `Rogrep Page.dc.html` reference. Edit `apps/ironscribe/components/rogrep-page.tsx`.
>
> Before you write anything, check two facts and report them: is `rogrep` published to crates.io, and does the Homebrew tap exist? If either is no, that install row does not ship — promote the source build instead. A command that fails when pasted is worse than no command.
>
> Then: the three-row install group, the repo-facts strip, "Built in the open" with the activity strip, and delete the `PLACEHOLDER VALUES` stats table that is currently live in section 04.
>
> Wire stars/contributors/commits/last-commit as a build-time GitHub API fetch per Task 5. Every call in try/catch, falling back to omitting the value — a rate-limited API must not fail the build or render a zero. Hide the whole activity strip when no figure resolves.
>
> Keep the cass benchmark exactly as it is, including all caveats.
