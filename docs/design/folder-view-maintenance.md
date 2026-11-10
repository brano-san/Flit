# Folder views and Linux storage

## Adaptive

Adaptive chooses a concrete layout; drawing then follows the same code as an explicitly selected layout, including file sizes in M Icons.

An explicit desktop folder type takes precedence: Pictures and Videos select Grid; Documents, Music and Generic select Details. Without that metadata, Grid is selected when image and video files make up more than 60% of the visible base listing. Directories count towards the denominator. Other folders use Details.

The previous decision is cached per folder while a new listing loads. The completed listing determines the final layout immediately and updates that cache. Intermediate batches do not repeatedly switch layouts. The cache is bounded to 4096 folders.

## Folder statistics

Folder size, nested-folder count and nested-file count have independent switches on the Data and Performance settings page. In Details and Tree, a scan requires at least one enabled statistic whose column is visible. Other layouts request folder sizes only. Disabled statistics do not display cached values or loading indicators. Hiding every enabled statistic cancels pending work. Multiple enabled statistics share one traversal and the same cached result.

The two count switches live in an appended state record. Older preferences initialize both from the former shared size switch, preserving the user's previous choice. The size switch keeps its existing shortcut and menu command.

Watcher changes and completed file operations invalidate affected directories. Other calculated directories retain their cached statistics. New and invalidated rows enter the existing loading animation. Repeated requests do not duplicate pending walks; Alt+R explicitly invalidates cached statistics for the current view.

The provider checks SSD, HDD and network preferences before walking each requested directory, including directories that are themselves mount points. Linux reads the block device's rotational flag, follows partitions to the whole disk, and inspects backing devices for device-mapper and RAID. Filesystems with a virtual device number can fall back to their mounted block-device source. Mixed backing devices containing an HDD are treated as HDDs. Unknown local storage follows the SSD preference.

Mounted NFS, SMB/CIFS, SSHFS and the other recognized network filesystems use the network preference. GVfs mounts also use that preference. The deepest containing mount wins, with component boundaries respected.

## Linux installer

The archive's `install.sh` installs into `$HOME/.local` by default; `PREFIX` overrides that location. It installs the executable, desktop launcher, icons, licenses and D-Bus template, then refreshes desktop and icon caches when those utilities are available. It prints each stage and a PATH hint when needed.

`--dbus-activation` additionally registers Flit as the user D-Bus FileManager1 service. `--uninstall` removes Flit's installed files and its matching service, preserving unrelated files. No sudo or system-wide installation is required for the default prefix.

`sh scripts/test-install-linux.sh` exercises installation, reinstallation with D-Bus, uninstallation, invalid arguments and a prefix containing spaces and an ampersand in an isolated temporary directory.

## Safety and saved preferences

Extract and delete archive uses Smart extraction and sends the archive to the trash only after successful completion. Cancellation, extraction errors and skipped entries preserve the original archive.

Git integration defaults to disabled. Git integration and Places ordering/hidden entries are stored in an appended state record, retaining compatibility with earlier state records. Places can be reordered by dragging or menu commands, hidden individually, and restored from the section menu.

The Quick Access filter is hidden in the normal sidebar; Alt+F temporarily opens it, and Escape closes it. Context-menu expansion keeps the menu's position and uses scrolling when it reaches the screen edge.
