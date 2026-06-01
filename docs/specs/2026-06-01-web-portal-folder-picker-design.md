# Web Portal: Folder Picker for Adding a Repo

Date: 2026-06-01
Status: Draft (awaiting review)

## Goal

Replace the "type an absolute path" text input in the portal's add-repo flow with
an in-browser folder browser. The user clicks "Add repository", navigates their
filesystem in a modal, and selects a folder to register. The manual text input is
removed entirely.

## Constraint That Shapes the Design

Browsers cannot give a web page the absolute filesystem path of a folder:

- `<input type="file" webkitdirectory>` exposes only `webkitRelativePath` (relative
  to the chosen folder) and file names, never the folder's absolute path.
- The File System Access API `showDirectoryPicker()` returns an opaque
  `FileSystemDirectoryHandle` with no `.path`, and is Chromium-only.

The server's `registry.register()` needs a real absolute path. So selection must be
driven by the server, which can read the local filesystem. Since this is a
localhost-only macOS daemon, the chosen approach is a server-backed directory-listing
endpoint plus an in-browser navigator (no native OS dialog, no dependence on launchd
GUI/focus/TCC behavior).

## Architecture

Two pieces:

1. Backend: a new read-only `GET /api/fs/list` endpoint that lists the subdirectories
   of a given absolute path and returns absolute paths the client can navigate and
   submit.
2. Frontend: a `FolderPickerDialog` modal (navigate folders, pick one) wired into
   `ReposView`, reusing the existing `useAddRepo` mutation to register the chosen path.

```
[ReposView] --click "Add repository"--> [FolderPickerDialog]
   FolderPickerDialog --GET /api/fs/list?path=...--> [handle_fs_list] --> listing JSON
   FolderPickerDialog --"Add this folder"--> useAddRepo --POST /api/repos {path}--> register()
```

## Backend

### New module: `src/server/api/filesystem.rs`

(Named `filesystem` rather than `fs` to avoid confusion with `std::fs`. Route path is
still `/api/fs/list`.)

#### Endpoint contract

`GET /api/fs/list`

Query params:

- `path` (optional): absolute directory to list. When omitted or empty, defaults to
  `$HOME` (via `std::env::var("HOME")`, falling back to `/` if unset).
- `show_hidden` (optional bool, default `false`): include dot-directories.

Success response (`200`):

```json
{
  "path": "/Users/dikrana",
  "parent": "/Users",
  "entries": [
    { "name": "Documents", "path": "/Users/dikrana/Documents", "has_git": false, "hidden": false },
    { "name": "myrepo",     "path": "/Users/dikrana/myrepo",     "has_git": true,  "hidden": false }
  ]
}
```

- `path`: the canonicalized directory that was listed (so the client always works with
  canonical absolute paths).
- `parent`: parent directory, or `null` when `path` is the filesystem root `/`.
- `entries`: subdirectories only (files excluded), sorted case-insensitively by name.
  - `has_git`: `true` when `<entry>/.git` exists (a hint that the folder is a repo).
  - `hidden`: `true` when the name starts with `.`.

Error responses (JSON `{ "error": "..." }`):

- `400` path is not valid UTF-8, path does not exist, or path is not a directory.
- `403` permission denied reading the directory.

### Listing logic (pure, unit-testable)

```rust
struct FsEntry { name: String, path: Utf8PathBuf, has_git: bool, hidden: bool }
struct FsListing { path: Utf8PathBuf, parent: Option<Utf8PathBuf>, entries: Vec<FsEntry> }
enum FsListError { NotFound, NotADirectory, PermissionDenied, NonUtf8 }

fn list_directory(path: &Utf8Path, show_hidden: bool) -> Result<FsListing, FsListError>
```

Behavior:

1. Canonicalize `path` with `dunce::canonicalize` (resolves symlinks, validates
   existence). Map `io::ErrorKind::NotFound` -> `NotFound`, `PermissionDenied` ->
   `PermissionDenied`. Non-UTF-8 canonical result -> `NonUtf8`.
2. If not a directory -> `NotADirectory`.
3. `read_dir` the directory (map `PermissionDenied`). For each entry:
   - Skip entries whose names are not valid UTF-8 (cannot be represented as
     `Utf8PathBuf`; skipped silently rather than failing the whole listing).
   - Keep only directories. Use `path.is_dir()` so symlinked directories are included.
   - `hidden = name.starts_with('.')`; if `hidden && !show_hidden`, skip.
   - `has_git = entry_path.join(".git").exists()`.
4. Sort entries case-insensitively by `name`.
5. `parent = path.parent()` (`None` at `/`).

The handler is thin glue: parse params, apply the `$HOME` default, call
`list_directory`, and map `FsListError` to the JSON error responses above. All
filesystem work runs on `tokio::task::spawn_blocking` (consistent with the existing
repo-stats reads in `repos.rs`, since these are synchronous syscalls).

### Routing

Register in `src/server/api/mod.rs`:

- `mod filesystem;` + `use filesystem::handle_fs_list;`
- `.route("/api/fs/list", get(handle_fs_list))`
- Add to the module doc-comment route list.

### Security

- The endpoint is read-only (directory listing) and already bound to 127.0.0.1 with
  Origin-header rejection (same defense as every other `/api` route). No new exposure
  beyond what the daemon already has: it reads the user's own filesystem on the user's
  own machine.
- No base-dir restriction: the user must be able to navigate anywhere to find a repo
  (since the text-entry escape hatch is removed). This is acceptable for a local
  single-user tool. Errors (permission denied, missing) are surfaced as JSON, never
  panics.

## Frontend

### New dependency + component

- Add `@radix-ui/react-dialog` (a generic modal; only `@radix-ui/react-alert-dialog`
  exists today, which is semantically a confirmation primitive and a poor fit for a
  scrollable navigator).
- Add `ui/src/components/ui/dialog.tsx` (standard shadcn Dialog wrapper) styled to
  match the existing terminal aesthetic of `card.tsx` / `alert-dialog.tsx`.

### API layer

- `ui/src/api/types.ts`: add `FsEntry` and `FsListing` types mirroring the JSON above.
- `ui/src/api/fs.ts`: `listDir(path?: string, showHidden?: boolean, signal?) =>
  apiGet<FsListing>("/fs/list?...")` building a URL-encoded query string.
- `ui/src/features/repos/useFsList.ts`: `useFsList(path, showHidden, enabled)` react-query
  hook keyed by `["fs", path ?? "", showHidden]`, with `placeholderData: keepPreviousData`
  so navigating does not flash an empty list.

`useAddRepo` is reused unchanged (it already POSTs `{ path }` and invalidates `["repos"]`).

### `FolderPickerDialog.tsx`

State:

- `currentPath: string | undefined` (starts `undefined` -> first `listDir()` call with no
  path returns the `$HOME` listing, whose `path` field seeds `currentPath`).
- `showHidden: boolean` (default `false`).
- `lastGoodPath: string` ref, updated on every successful listing, used as the fallback
  target when navigating into an unreadable folder errors.

Layout (top to bottom):

- Title: "Select a repository folder".
- Breadcrumb derived from the last successful `listing.path`; each segment clickable to
  jump to that ancestor. An "Up" control navigates to `listing.parent` (disabled when
  `parent === null`).
- "Show hidden folders" toggle.
- Scrollable list of `listing.entries`. Each row is a button: click -> descend
  (`setCurrentPath(entry.path)`). A small "git" badge renders when `has_git`; hidden
  entries render dimmer. Empty directory -> "no subfolders" placeholder.
- Footer: shows the current `listing.path` and an "Add this folder" button that calls
  `addRepo.mutate(listing.path)` and closes the dialog on success. Disabled while the
  listing is loading or the add is pending.

Interaction model: navigate *into* the folder you want, then "Add this folder" registers
the currently-viewed directory. This is unambiguous (no select-vs-descend confusion) and
matches common folder-open panels.

Error handling:

- Listing error (e.g. permission denied on a folder the user clicked into): show an
  inline banner ("cannot open <path>: <reason>") with a "Go back" action that resets
  `currentPath` to `lastGoodPath`. `keepPreviousData` keeps the prior entries visible
  underneath.
- Add error (surfaced by `useAddRepo`): show the message in the footer; the dialog stays
  open so the user can retry or pick a different folder. (`register()` is idempotent, so
  re-adding an existing repo simply succeeds and the list refreshes.)

Accessibility (WCAG AA, per project design context): dialog has an accessible title,
Esc-to-close and focus trap (Radix), keyboard-focusable rows, and `aria-label`s on the
Up/breadcrumb/toggle controls.

### `ReposView.tsx` change

Replace the `AddRepoForm` text-input block (`ReposView.tsx:47-78`) with an "Add
repository" button that opens `FolderPickerDialog`. Everything else in `ReposView`
(repo list, reindex, drop) is unchanged.

## Testing (TDD)

### Backend (`cargo test`)

Unit tests for `list_directory` against `tempfile::tempdir()` fixtures:

- Lists subdirectories, sorted case-insensitively; files are excluded.
- `parent` is computed; nested dir has a parent, and the logic returns `None` for `/`
  (assert via a dir whose parent is known, plus a direct check that `/` yields `None`).
- Hidden dirs excluded by default, included when `show_hidden = true`; `hidden` flag set.
- `has_git` is `true` when a `.git` subdirectory exists, else `false`.
- Non-existent path -> `NotFound`; a file path -> `NotADirectory`.

A handler-level test (mirroring the style in `repos.rs`) exercises param defaulting and
`FsListError` -> HTTP-status mapping for an explicit `?path=` against a temp dir.

### Frontend (`bun test`, Testing Library + happy-dom)

`FolderPickerDialog.test.tsx` (mocking `fetch` like `AddRepo.test.tsx`):

- Opening the dialog issues `GET /api/fs/list` and renders the returned entries.
- Clicking an entry issues `GET /api/fs/list?path=<entry>` (descend).
- "Add this folder" POSTs `{ path: <current listing.path> }` to `/api/repos` and the
  dialog closes.
- A listing error renders the banner and "Go back" restores the previous listing.

`AddRepo.test.tsx` is rewritten to drive the folder-picker flow (the old text-input
assertions are removed, since that input no longer exists).

## Out of Scope (YAGNI)

- Native OS folder dialog (Option A, rejected for launchd/TCC fragility).
- Virtualizing the folder list (plain max-height scroll; revisit only if a real
  directory proves too large to render smoothly).
- Multi-select / adding several folders at once.
- Filtering or search within the folder list.
- Restricting navigation to a configured root.
```
