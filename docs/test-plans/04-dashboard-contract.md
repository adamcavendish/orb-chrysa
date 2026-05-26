# Dashboard Contract Test Plan

**Date**: 2026-05-25
**Type**: Product contract test plan
**Source**: Dashboard product behavior, active UI mockups, and implementation
contracts
**Scope**: Shared dashboard navigation, routing, tables, filtering, copy,
destructive actions, selection mode, localization, theme behavior,
visual/accessibility behavior, and API conventions.

---

## Product Contract Summary

The Orb Chrysa Container Registry dashboard is an operational console with five
top-level sections:

```text
Overview
Repositories
Mirror
Proxy Cache
Cluster
```

All feature pages share hash routing, compact tables, server-side
filtering/sorting, copy affordances, visible cancellation for destructive
actions, local static translations, dark/light/system themes, and operational
styling.

## Features Tested

| Feature | Tests | Priority |
|---------|-------|----------|
| Canonical navigation and removed sections | D1 | P0 |
| Hash routes and deep links | D2 | P0 |
| Page chrome and freshness | D3 | P1 |
| Shared table states | D4 | P0 |
| Pagination contract | D5 | P0 |
| Filtering and sorting contract | D6 | P0 |
| Copy affordance standard | D7 | P1 |
| Destructive action policy | D8 | P0 |
| Selection mode | D9 | P1 |
| Theme modes and persistence | D10 | P0 |
| Localization and RTL | D11 | P0 |
| Visual and responsive behavior | D12 | P1 |
| Accessibility | D13 | P1 |
| Dashboard API conventions | D14 | P0 |
| Session identity display | D15 | P0 |

## Tests

### D1. Canonical Navigation

**Steps**:
1. Open the dashboard root.
2. Inspect the top navigation.
3. Navigate through every top-level item.

**Expected**:
- Exactly these top-level labels are visible: Overview, Repositories, Mirror,
  Proxy Cache, Cluster.
- Current section is highlighted.
- Removed top-level labels are not present: Helm Charts, Warm Images, Mirror
  Rules, Mirror Jobs.
- Mirror-specific rules/jobs are reachable as Mirror subtabs, not top-level nav.

### D2. Hash Routes And Deep Links

**Steps**:
1. Load each hash route directly:
   - `#/overview`
   - `#/repos`
   - `#/repos/platform/orb-chrysa-api`
   - `#/mirror`
   - `#/proxy-cache`
   - `#/cluster`
2. Refresh each route.
3. Use browser back/forward between routes.

**Expected**:
- Each route loads without server 404.
- Refresh preserves the current page.
- Repository names with slashes are decoded as one repository name.
- Back/forward preserves current route and practical filter/sort/page state.
- Deep-linked routes do not emit browser console errors.

### D3. Page Chrome And Freshness

**Steps**:
1. Load each dashboard page.
2. Wait for a successful refresh.
3. Simulate one failed refresh while stale data is visible.

**Expected**:
- Page title remains visible in loading, error, and loaded states.
- Nested pages include a breadcrumb such as Back to Repositories.
- Freshness shows the last successful API refresh time.
- Failed refresh keeps stale data visible and shows an error banner.

### D4. Shared Table States

**Steps**:
1. Exercise loading, empty, error, loaded, and connection-lost states for each
   table page.
2. Force three consecutive refresh failures.

**Expected**:
- Loading state uses a stable spinner or skeleton in the table area.
- Empty state includes contextual copy and create action when applicable.
- Error banner appears at the top without erasing stale data.
- After three consecutive failures, the page shows a full-page retry affordance.

### D5. Pagination Contract

**Steps**:
1. Load a list with more than one page of data.
2. Change page size through 25, 50, 100, and 200.
3. Use Prev and Next.
4. Inspect network requests.

**Expected**:
- Pagination text follows the shape:
  `Showing 1-50 of 1,234,567    [50 v]  <- Prev  1  Next ->`.
- Default page size is 50.
- Page size choices are 25, 50, 100, and 200.
- List APIs use server-side pagination and RFC 5988 Link headers.

### D6. Filtering And Sorting Contract

**Steps**:
1. Apply search text and every visible filter on each list page.
2. Apply each sort option.
3. Remove filters through chips.
4. Inspect network requests.

**Expected**:
- Search requests debounce.
- Filtering and sorting are server-side for pageable lists.
- Applying filters resets to the first page and preserves page size.
- Active non-default filters appear as removable chips.
- Removing chips updates controls and reloads results.
- Empty filtered results explain that no rows match.

### D7. Copy Affordance Standard

**Steps**:
1. Copy representative values: repository name, manifest digest, config digest,
   subject digest, Helm install snippet, mirror/proxy local prefix.
2. Confirm clipboard content.
3. Observe feedback.

**Expected**:
- Copy uses explicit controls, not row navigation or expansion targets.
- Full values are copied even when table text is shortened.
- Inline feedback shows Copied for about one second.
- Toasts are used only when inline feedback would be distant.

### D8. Destructive Action Policy

**Steps**:
1. Trigger every destructive action: tag delete, repository delete, digest
   delete, batch delete, rule delete, cache delete, member leave/remove.
2. Try Cancel, Escape, outside click, and Confirm.
3. Simulate request failure.

**Expected**:
- Tag chip delete uses two-step inline confirm.
- Single-object deletes show warning/modal with Cancel and Confirm delete.
- Batch deletes show object/cascade counts.
- Large batch deletes require typing `delete`.
- Cancel/Escape returns to non-warning state.
- In-flight confirm buttons are disabled.
- Failure keeps the object visible and shows an error.

### D9. Selection Mode

**Steps**:
1. Enter selection mode on a supported table.
2. Select rows, select all visible, cancel selection.
3. Click a row while selection mode is active.

**Expected**:
- Selection mode starts only through a Select button.
- Rows gain checkboxes.
- Row click no longer performs the default row action.
- Sticky action bar shows selected count, actions, and Cancel.
- Select all applies only to visible rows on the current filtered page.

### D10. Theme Modes And Persistence

**Steps**:
1. Open the dashboard with no saved theme.
2. Switch theme through System, Dark, and Light.
3. Refresh the current route and deep-link directly to `#/repos`,
   `#/mirror`, `#/proxy-cache`, and `#/cluster`.
4. Inspect `localStorage` and document theme state.

**Expected**:
- Default theme is System.
- Selected theme is stored as `orb-chrysa.theme`.
- Theme changes apply immediately without changing route or table state.
- Light theme keeps the same operational layout, density, and workflows.
- Light theme preserves readable contrast for page backgrounds, cards, tables,
  badges, filter controls, topbar controls, modals, and destructive actions.
- The topbar exposes exactly one compact language selector and one segmented
  System/Light/Dark theme control without label wrapping or overflow at desktop
  and mobile widths.
- Refresh and deep links preserve the selected theme.

### D11. Localization And RTL

**Steps**:
1. Clear saved locale and load the dashboard with browser language set to a
   supported locale.
2. Use the topbar selector to switch through English, Chinese, and Arabic.
3. Refresh and deep-link to each page while Arabic is selected.
4. Inspect document `lang` and `dir`.

**Expected**:
- Supported locales are `en`, `es`, `fr`, `de`, `zh`, `ja`, `ko`, `ar`, `pt`,
  `ru`, `it`, `nl`, `hi`, `tr`, and `vi`.
- Selected locale is stored as `orb-chrysa.locale`.
- Dashboard strings update without full page reload.
- Arabic sets `dir="rtl"`; all other supported locales use `ltr`.
- RTL does not break the topbar, nav, tables, chips, row actions, modals,
  form grids, advanced sections, or expanded rows.
- Technical values such as digests, cron expressions, endpoints, and repository
  names remain readable and copyable when the document is RTL.
- Backend API errors remain English for v1. Dashboard chrome, page titles,
  common actions, and fallback messages are translated; longer operational copy
  and technical media-type text may fall back to English.

### D12. Visual And Responsive Behavior

**Steps**:
1. Capture desktop and mobile screenshots of every dashboard page in dark and
   light themes.
2. Open modals, filter popovers, warning states, and expanded rows.
3. Resize between mobile and desktop widths.

**Expected**:
- UI uses compact tables, frosted panels, and restrained status colors in both
  light and dark themes.
- Ordinary cards and controls use 8px or smaller radius unless design system
  says otherwise.
- No text overlaps or clips in buttons, badges, chips, modals, or table cells.
- Tables remain readable through horizontal scroll or responsive row layout.
- No browser console errors appear while navigating active routes in dark,
  light, system, English, Chinese, or Arabic.

### D13. Accessibility

**Steps**:
1. Navigate dashboard controls by keyboard.
2. Open/close modals and popovers.
3. Inspect accessible names for icon-only buttons.
4. Exercise language and theme selectors.

**Expected**:
- All controls have accessible names.
- Language and theme selectors have accessible labels.
- Modal focus is trapped and restored on close.
- Keyboard users can copy, expand, confirm/cancel, and navigate tabs.
- Icon-only controls have tooltips or accessible labels.
- Light and dark themes preserve readable contrast for text, badges, destructive
  actions, focus rings, and disabled states.

### D14. Dashboard API Conventions

**Steps**:
1. Inspect all dashboard network requests.
2. Fetch list and detail endpoints containing credentials.
3. Force representative API errors.

**Expected**:
- Dashboard uses `/api/v1/*` for enriched UI data.
- OCI clients continue to use `/v2/*`.
- List endpoints are paginated.
- List/detail responses do not return secrets.
- Credential fields are write-only.
- Errors use structured JSON with stable machine code and human message.
- Mutations return enough data for UI reconciliation when practical.

### D15. Session Identity Display

**Steps**:
1. Sign in through OIDC as `admin`.
2. Fetch `GET /api/v1/session`.
3. Open the account chip and Access page session panel.
4. Repeat with `developer`.

**Expected**:
- Session JSON contains `subject`, `username`, `display_name`, `email`,
  `groups`, `scopes`, and `token_type`.
- Session JSON does not contain `user_id`.
- The account chip and menu use `display_name`, then `username`, then `email`,
  then `subject` as the display label.
- Avatar initials derive from the human display label, so Admin User renders
  `AU` and Developer User renders `DU`.
- PAT ownership and token revocation continue to key on stable `subject`, not
  display text.

## Coverage Map

| Contract Area | Tests |
|---------------|-------|
| Canonical navigation | D1 |
| Hash routes | D2 |
| Page chrome | D3 |
| Shared table behavior | D4, D5 |
| Filtering and sorting | D6 |
| Copy affordances | D7 |
| Destructive action policy | D8 |
| Selection mode | D9 |
| Visual design | D10, D12 |
| Localization | D11 |
| Accessibility and responsive behavior | D11, D12, D13 |
| API conventions | D14 |
| Session identity | D15 |
