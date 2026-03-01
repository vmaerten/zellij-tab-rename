# CWD Resolution Architecture

## Problem

The plugin renames tabs based on the focused pane's CWD. Zellij delivers CWD information through two mechanisms with different timing guarantees, creating several edge cases.

## Event Sources

| Event | Payload | When emitted |
|---|---|---|
| `PaneUpdate` | Full pane manifest (id, tab, focus, title) — **no CWD** | On any pane state change (create, focus, close, title change) |
| `CwdChanged` | `(PaneId, PathBuf)` | When a pane's CWD **changes** — NOT on initial CWD |

Key insight: `CwdChanged` fires on *change*, not on *set*. A new pane that keeps its initial CWD will never emit `CwdChanged`.

## Resolution Strategies

Three complementary mechanisms ensure CWD is always available:

### 1. `CwdChanged` handler (normal `cd` case)

**Trigger**: user runs `cd` in a terminal pane.

```
CwdChanged(pane_id, cwd)
  → pane already in pane_info? → update cwd, rename if focused
  → pane NOT in pane_info?    → buffer in pending_cwds
```

### 2. `pending_cwds` drain (race condition)

**Trigger**: `CwdChanged` arrives *before* `PaneUpdate` for a new pane (observed during session restore).

```
PaneUpdate arrives
  → for each pane: check pending_cwds.remove(pane_id)
  → if found: apply CWD, mark tab for rename
```

### 3. `get_pane_cwd()` active fetch (new tab case)

**Trigger**: `PaneUpdate` shows a focused terminal pane with empty CWD (no `CwdChanged` received, no pending entry).

```
PaneUpdate arrives
  → focused terminal pane with empty cwd?
  → call get_pane_cwd(pane_id) synchronously
  → if Ok(cwd) and non-empty: store it
```

## Scenarios

### S0: Initial startup (permissions arrive after events)

```
1. load()              → request_permission + subscribe
2. CwdChanged          → got_permissions = false → buffered_events.push()
3. PaneUpdate          → got_permissions = false → buffered_events.push()
4. PermissionRequestResult(Granted)
   → got_permissions = true
   → replay buffered_events via process_event()
     → CwdChanged  → handle_cwd_changed() → pending_cwds (pane not yet known)
     → PaneUpdate  → handle_pane_update() → drains pending_cwds, get_pane_cwd() fallback
   → tab renamed ✓
```

### S1: User opens a new tab

```
1. PaneUpdate        → new pane appears, focused, cwd = ""
2. pending_cwds      → nothing buffered
3. get_pane_cwd()    → fetches initial CWD ✓
4. tabs_to_rename    → tab renamed
```

### S2: User runs `cd /some/path`

```
1. CwdChanged        → pane known, cwd updated
2. focused check     → pane is focused → rename_tab ✓
```

### S3: Session restore (CwdChanged arrives first)

```
1. CwdChanged        → pane NOT in pane_info → buffered in pending_cwds
2. PaneUpdate        → pane appears, pending_cwds drained → cwd applied ✓
3. tabs_to_rename    → tab renamed
```

### S4: Session restore (PaneUpdate arrives first)

```
1. PaneUpdate        → pane appears, cwd = "", no pending
2. get_pane_cwd()    → fetches CWD ✓
3. tabs_to_rename    → tab renamed
4. CwdChanged        → arrives later, updates cwd (no-op if same)
```

### S5: Focus switch between panes in same tab

```
1. PaneUpdate        → new focused pane detected (prev_focused != pane_id)
2. pane already has cwd (from earlier CwdChanged or get_pane_cwd)
3. tabs_to_rename    → tab renamed to new pane's cwd ✓
```

### S6: Tab closed

```
1. TabUpdate         → tab position absent from active set
2. focused_panes     → entry removed (retain)
3. pane_info         → entries removed (retain)
4. current_tab_names → entry removed (retain)
```

### S7: Plugin pane focused (e.g. file picker)

```
1. PaneUpdate        → pane.is_plugin = true
2. All CWD logic     → skipped (guarded by !pane.is_plugin) ✓
```

### S8: `get_pane_cwd` returns error or empty path

```
1. get_pane_cwd()    → Err(_) or Ok("")
2. cwd stays empty   → tab NOT renamed (no empty name)
3. Next CwdChanged   → will fill it in later ✓
```

## Ordering Guarantees

- `pending_cwds` is checked *before* `get_pane_cwd()` — if both sources have data, the event-driven one wins (fresher).
- `get_pane_cwd()` only fires when cwd is still empty after pending drain.
- `tabs_to_rename` is a `HashSet` — a tab is renamed at most once per `PaneUpdate` cycle regardless of how many sources contributed.

## Cleanup

| Structure | Cleaned by |
|---|---|
| `pane_info` | `handle_pane_update` — retain only seen panes |
| `focused_panes` | `handle_tab_update` — retain only active tabs |
| `current_tab_names` | `handle_tab_update` — retain only active tabs |
| `pending_cwds` | Drained on use in `handle_pane_update` |
| `git_root_cache` | Never cleaned (bounded by number of unique CWDs visited) |
| `pending_git_lookups` | Drained on `RunCommandResult` |
