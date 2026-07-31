# TODO — crow (zed fork)

Fork-local tracking list. Upstream is `zed-industries/zed` (`upstream` remote);
our work is on `origin` (`odellus/zed`). This file is for *our* bugs and tasks,
not upstream's.

The thesis we're shaking out: **the ACP client is a first-class editor panel
citizen.** Most of the rough edges live at that seam.

---

## Bugs

### ACP panel message routing — input delivered to the wrong session
- **Date:** 2026-07-31
- **Observed:** A user message typed for session A (`stirring-realistic-antelope`)
  was delivered to a different live session B (`chivalrous-fragrant-firefly`).
  No error, no warning — session B's agent just answered a message it was never
  meant to receive, and session A saw nothing. Silent misrouting.
- **Expected:** Input entered in a panel targets *that panel's* session. When
  multiple ACP sessions are live, "where does the next message go" must be
  well-defined (foreground/focused panel should win) and a misroute should fail
  loudly, not silently.
- **Suspected area:** session targeting / input dispatch in the ACP panel layer
  (`crates/crow-acp/`, panel ↔ `AcpSession` wiring in `src-tauri/`). Likely no
  explicit focus/foreground signal is being used to pick the target session.
- **Open questions:**
  - What *is* the current routing rule (last-active? last-created? arbitrary)?
  - Is there a foreground/focus signal available from the workbench to key off?
  - Should a session be able to receive a message "addressed" to another, or
    should that be a hard error?
- **Related class of corner cases:** ambiguous continuation across N live
  sessions; agent identity under delegation ("what did *you* say" when a session
  has agents a1–a3).

---

## Tasks

- [ ] **Upstream watch** — near-daily digest of new `zed-industries/zed`
      activity (new commits on `upstream/main`, new tags). Implemented as
      `script/upstream-watch` + cron; digest lands in
      `~/.crow/notes/dev/upstream-zed-digest.md`.
