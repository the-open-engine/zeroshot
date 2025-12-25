# Two-Level Navigation - Implementation Summary

## Overview

Completely redesigned TUI layout with separate views for overview and detail modes:

1. **Overview mode** (default): ONLY clusters + stats - clean, focused view
2. Press Enter → **Detail mode**: ONLY agents + logs for selected cluster
3. Press Escape → Return to overview

## User Experience

### Overview Mode (Default)

- **ONLY visible:** Large clusters table (16 rows) + system stats sidebar
- **Hidden:** Agent table and logs (completely invisible)
- Clean, spacious layout focusing on cluster selection
- Help text: `[Enter] View  [↑/↓] Navigate  [k] Kill  [s] Stop  [l] Logs  [r] Refresh  [q] Quit`

### Detail Mode (After pressing Enter)

- **ONLY visible:** Full-width agents table (9 rows) + full-width logs (9 rows)
- **Hidden:** Clusters table and stats box (completely invisible)
- Dedicated space for monitoring single cluster in depth
- Help text: `[Esc] Back  [k] Kill  [s] Stop  [e] Export  [l] Logs  [r] Refresh  [q] Quit`

### Navigation Flow

```
Overview (ONLY clusters + stats)
    ↓ Enter
Detail (ONLY agents + logs)
    ↓ Escape
Overview (ONLY clusters + stats)
```

## Implementation Details

### Layout Design

**Overview mode layout** (`src/tui/layout.js`):

- Clusters table: rows 0-16 (16 rows), cols 0-8
- Stats box: rows 0-16 (16 rows), cols 8-12
- Help bar: rows 18-20
- Agents/logs: **hidden** (`.hide()` called on initialization)

**Detail mode layout** (`src/tui/layout.js`):

- Agents table: rows 0-9 (9 rows), cols 0-12 (full width)
- Logs box: rows 9-18 (9 rows), cols 0-12 (full width)
- Help bar: rows 18-20
- Clusters/stats: **hidden** (`.hide()` called on mode switch)

### State Management

**New state in TUI class (`src/tui/index.js`):**

```javascript
this.viewMode = 'overview'; // or 'detail'
this.detailClusterId = null; // cluster ID when in detail mode
```

### Keybindings

**Enter key** (`src/tui/keybindings.js` lines 14-37):

- Checks if in overview mode with clusters available
- Sets `viewMode = 'detail'` and `detailClusterId`
- **Hides** clusters table and stats box (`.hide()`)
- **Shows** agents table and logs box (`.show()`)
- Updates help text
- Clears old messages

**Escape key** (`src/tui/keybindings.js` lines 39-59):

- Checks if in detail mode
- Sets `viewMode = 'overview'` and `detailClusterId = null`
- **Shows** clusters table and stats box (`.show()`)
- **Hides** agents table and logs box (`.hide()`)
- Updates help text
- Clears messages

### Conditional Rendering

**Cluster state updates** (`src/tui/index.js` lines 107-119):

```javascript
if (this.viewMode === 'detail' && this.detailClusterId) {
  // Show agents for detail cluster
  const status = this.orchestrator.getStatus(this.detailClusterId);
  this.renderer.renderAgentTable(status.agents, this.resourceStats);
} else if (this.viewMode === 'overview') {
  // Don't show agents in overview
  this.renderer.renderAgentTable([], this.resourceStats);
}
```

**Resource stats updates** (`src/tui/index.js` lines 130-137):

- Same conditional logic as above
- Only renders agents in detail mode

## Testing

### Automated Tests

**Test 1: `tests/tui-integration.test.js`**

- Basic TUI startup
- Data loading
- Module integration
- ✅ PASSING

**Test 2: `tests/tui-navigation-test.js`**

- Initial state verification
- Enter detail view
- Verify agents shown
- Return to overview
- Conditional rendering logic
- ✅ PASSING

### Manual Testing

**Run the manual test:**

```bash
chmod +x tests/tui-keybindings-manual-test.js
node tests/tui-keybindings-manual-test.js
```

**Instructions:**

1. Press ↑/↓ or j/k to navigate clusters
2. Press Enter to drill into detail view → clusters/stats hide, agents/logs appear
3. Press Escape to return to overview → clusters/stats reappear, agents/logs hide
4. Verify help text updates correctly

## Files Modified

| File                     | Changes                                                 |
| ------------------------ | ------------------------------------------------------- |
| `src/tui/index.js`       | Added viewMode state, conditional rendering             |
| `src/tui/keybindings.js` | Added Enter/Escape handlers, widget visibility toggling |
| `src/tui/layout.js`      | Updated help text to show Enter key                     |
| `src/tui/CHANGES.txt`    | Documented feature and technical changes                |

## Files Created

| File                                   | Purpose                                 |
| -------------------------------------- | --------------------------------------- |
| `tests/tui-navigation-test.js`         | Automated test for two-level navigation |
| `tests/tui-keybindings-manual-test.js` | Interactive manual test                 |
| `src/tui/TWO-LEVEL-NAVIGATION.md`      | This document                           |

## Performance Impact

- **Startup:** No impact (viewMode check is O(1))
- **Rendering:** Slight improvement in overview mode (no agent data fetching)
- **Memory:** Minimal increase (2 new state variables)

## Known Limitations

None. Feature is complete and tested.

## Usage

```bash
# Start TUI (shows overview by default)
zeroshot watch

# In overview:
#   - Use ↑/↓ or j/k to select cluster
#   - Press Enter to drill into detail view

# In detail:
#   - View agents and logs for selected cluster
#   - Press Escape to return to overview
```

## Future Enhancements (Optional)

- Add breadcrumb showing current cluster in detail mode
- Add keybinding to jump directly to a cluster by ID
- Add "pinning" to keep detail view on specific cluster even when new clusters spawn
