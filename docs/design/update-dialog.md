# Update dialog

The update prompt uses the existing Form actions, keyboard focus and accessibility tree. Its header contains two copyable version rows. The footer keeps status, progress and action buttons outside the scrollable release notes. Copy feedback occupies the reserved status area, fades after two seconds and restores the underlying status after three seconds.

The dialog is bounded by the application window. Buttons use measured text widths and move to two rows when necessary. On exceptionally small windows the dialog scales down to keep the header and footer reachable. The primary action remains accented regardless of keyboard focus.

Release notes retain all lines, wrap to the viewport width and break words that are wider than the viewport. Bullets use hanging indentation; Markdown headings use a larger bold font and extra spacing. Wrapped layout is cached until the available width or scale changes. Only visible lines are painted, using the normal renderer clip and theme scrollbar colors.

Scrolling uses the application's Glide animation and supports wheel, thumb dragging, track paging, Page Up, Page Down, Home and End. Update installation, cancellation, skip-version and read-only handling continue through the existing update workflow.
