# 0006. Accessibility conformance target: WCAG 2.1 AA

**Status:** Accepted
**Date:** 2026-08-04
**Deciders:** Cahya Wirawan
**Related:** PRD §12 Q1, §9 (Phase 1 exit criterion), FR-027; invariant §7.3.9; ADR 0001; `spike/a11y-ime/FINDINGS.md`

## Context

PRD §12 Q1 asks directly: "WCAG 2.1 AA equivalent, or a specific procurement standard (Section 508 / EN 301 549)? This sets the bar the §9 Phase 1 audit measures against, and custom UI means it is earned widget by widget." Aurora renders its own UI on `wgpu` (ADR 0001) — nothing is accessible for free, so every widget has to individually earn whatever bar gets set here. Without a concrete target, "the Phase 1 accessibility audit" (§9's own exit criterion) has no yardstick, and invariant §7.3.9 ("every widget carries an `accesskit` node") has no way to say when a widget's *content* — not just its presence — is good enough.

This isn't a green-field choice. FR-027 already committed to one piece of it: every built-in theme's token set must pass **WCAG 2.1 AA contrast** (4.5:1 body text, 3:1 large text/UI boundaries), enforced in CI (`aurora-theme::contrast::check_gated_pairs`, `design/check_contrast.py`). Whatever this ADR decides has to be consistent with that already-shipping, already-tested piece, not invent a second, different bar alongside it.

There's also now real evidence to weigh, not just the abstract question. `spike/a11y-ime/FINDINGS.md` and this project's own live macOS testing (`aurora-app`, 2026-08-03–04) found: role/label/value/focus and CJK IME composition all reach VoiceOver correctly; live value-change announcements do not (a real, narrow bug, not structural); and — found later, with a real multi-panel tree — VoiceOver's linear keyboard navigation into deeply nested custom content is unreliable, while the same content is correctly discoverable via the Rotor and by the automated test suite. Windows (UIA) and Linux (AT-SPI) remain completely unverified by a human. Whatever target this ADR sets has to be honest about being *earned*, per the PRD's own phrasing, not assumed from the target's existence.

WCAG 2.1 is a *web content* standard by name and original scope. Aurora is desktop software, so nothing here claims literal WCAG conformance — the question, as PRD §12 itself frames it, is which bar to hold desktop work to as the closest available equivalent.

## Decision

Aurora's accessibility conformance target is **WCAG 2.1 Level AA's success criteria, applied to desktop software** — the same standard FR-027 already uses for contrast, extended to cover the rest of accessibility (keyboard operability, name/role/value, focus order and visibility, text alternatives, and so on), reinterpreted per criterion for a desktop context where a criterion is web-specific (e.g. page titles, viewport zoom) and therefore doesn't map directly.

This is deliberately not a claim of formal conformance to a procurement standard. It's chosen because it's also the substantive floor both named alternatives already build on:

- **Section 508** (US federal procurement), since its 2017 refresh, applies **WCAG 2.0 Level AA** success criteria to non-web software, not just web content.
- **EN 301 549** (EU accessibility standard, and the basis for the EU's 2025 European Accessibility Act, which reaches general commercial software, not just public procurement) is built on **WCAG 2.1 Level AA** success criteria extended to non-web ICT.

Meeting WCAG 2.1 AA's success criteria, honestly reinterpreted for desktop, is the one target that substantively satisfies all three framings PRD §12 named, without picking a bar scoped to only one jurisdiction or procurement context Aurora may never actually need to formally certify against.

The Phase 1 accessibility audit (§9's exit criterion) measures against this: for each widget in the component gallery, the WCAG 2.1 AA success criteria that have a real desktop equivalent, checked in every built-in theme and density mode where relevant (contrast already is; keyboard operability, name/role/value, and focus visibility need the same discipline extended to them).

## Alternatives considered

**Section 508 alone.** Rejected: a US federal procurement standard. Adopting it by name (rather than the WCAG 2.0 AA criteria it's built on) would tie the bar to one country's procurement law for a project with no current government-procurement target, and would leave the EU angle unaddressed for no real gain — the criteria overlap almost entirely with WCAG 2.1 AA anyway.

**EN 301 549 alone.** Rejected for the same reason in reverse: an EU-specific standard, and less globally recognized outside the EU than WCAG itself, even though EN 301 549 is itself WCAG-based. Naming WCAG 2.1 AA directly is the more legible choice for a project with no jurisdiction-specific mandate yet.

**No formal target — "reasonable effort," judged case by case.** Rejected: this is exactly what PRD §12 Q1 already identified as insufficient — "this sets the bar the §9 Phase 1 audit measures against," and an audit needs a checklist, not a vibe. It would also silently abandon the discipline FR-027 already applies to contrast, for no stated reason.

**WCAG 2.1 AAA (the stricter level).** Rejected: AAA includes several criteria not intended to be met by all content types (WCAG's own guidance), and neither Section 508 nor EN 301 549 requires it. AA is the level both procurement-adjacent standards actually converge on; AAA would set a bar nothing else asks for.

## Consequences

**Gained:** a concrete, externally-recognized bar the Phase 1 audit can actually check against, consistent with the contrast work already shipping. A widget or panel has a real definition of "done" for accessibility, not just "has an `accesskit` node." If Aurora ever does need to demonstrate formal Section 508 or EN 301 549 conformance (a future procurement or EAA-driven requirement), the WCAG 2.1 AA groundwork is the substantive majority of that work already done, not a restart.

**Cost:** WCAG's success criteria are written for *web content* — every criterion needs a real, honest judgment call about whether and how it maps to a desktop widget, and that reinterpretation work is itself real, per-widget effort (PRD's own "earned widget by widget," not automatic from choosing a target). This ADR does not grant formal Section 508 or EN 301 549 conformance on its own — an actual conformance claim (e.g. a VPAT/Accessibility Conformance Report) would need its own documentation and testing process this ADR doesn't attempt. And meeting a written success-criteria checklist is not the same thing as a screen reader actually working well in practice, which this project's own recent findings underline directly: the live-announcement gap and the deep-nesting keyboard-navigation gap are both real, reproducible failures that a criteria-only audit could plausibly miss without also doing the kind of live, human, multi-platform testing `spike/a11y-ime` and this session's own `aurora-app` testing already started.

**Follow-on work:** a Phase 1 accessibility audit checklist derived from WCAG 2.1 AA's success criteria, mapped explicitly to their desktop equivalent (or marked not-applicable with a stated reason) — extending what `check_contrast.py`/`check_gated_pairs` already do for contrast to keyboard operability, name/role/value, and focus visibility across the component gallery. Investigating the deep-nesting VoiceOver navigation gap (recorded in PLAN.md M1.8) is now explicitly in scope for that audit, not a side issue — WCAG 2.4.3 (Focus Order) and 4.1.2 (Name, Role, Value) are exactly the criteria that gap bears on. The still-open Windows/Linux legs of the accessibility spike (PLAN.md 0.4) are prerequisites for auditing on those platforms at all.

## Reconsider if…

- Aurora ever specifically targets US federal or EU public-sector procurement, or falls under the EAA's enforcement in practice — at that point, move from "WCAG 2.1 AA as the substantive bar" to formal Section 508/EN 301 549 conformance documentation (a VPAT/ACR), which is real, additional, dedicated work this ADR doesn't cover.
- WCAG 3.0 (in development at the time of this decision) reaches Recommendation status and gains the same broad recognition WCAG 2.1 currently has — its substantially different scoring model would be a genuine reason to revisit which version is the reference.
- The Phase 1 audit finds that WCAG's criteria, even honestly reinterpreted, systematically fail to catch the class of problem the live-announcement and deep-nesting findings represent — that would mean criteria-based auditing alone is insufficient for this project's UI shape, and the audit process (not just the target) needs to change.
