# fffinder control protocol, version 1

A running fffinder window keeps a live fff-search index of every search location. It answers queries about that index, and requests to show things, over a local connection, so shells and tools search the same index the window uses instead of building their own. `nu/fff.nu` is the bundled client.

The protocol is experimental. Endpoints may change between versions.

## Finding the server

The server describes itself in `server.json` inside the `run` folder of fffinder's data folder:

| System | Data folder |
|---|---|
| macOS | `~/Library/Application Support/fffinder` |
| Linux | `$XDG_DATA_HOME/fffinder`, else `~/.local/share/fffinder` |
| Windows | `%LOCALAPPDATA%\fffinder` |

`FFFINDER_DATA_DIR` replaces the data folder for both the app and clients.

On macOS and Linux the server listens on a Unix socket:

```json
{"protocol": 1, "pid": 4242, "socket": "/Users/me/Library/Application Support/fffinder/run/fffinder.sock"}
```

On Windows it listens on the loopback interface, and every request must carry the token as `Authorization: Bearer <token>`:

```json
{"protocol": 1, "pid": 4242, "url": "http://127.0.0.1:53817", "token": "1f0c…"}
```

The `run` folder and the socket are readable by the owner only. A missing file, or a connection that fails, means no window is running. Only the main window process serves; `fffinder --pick` never does. When fffinder starts and a live server already answers, the new process sends `POST /v1/open` with its arguments and exits, so there is one app and one index.

## Requests

HTTP/1.1, one request per connection; the server closes the connection after the response. `GET` parameters are URL-encoded in the query string (a `+` is a space) and may repeat where noted. `POST` bodies are JSON with a `Content-Length`, at most 16 MiB.

Responses are JSON. Single objects and errors carry a `Content-Length`. Lists are a JSON array streamed until the connection closes, so they carry none. `/v1/watch` streams one JSON object per line, also until the connection closes, and sends its headers before the first event. Responses are never chunked. Errors use a 4xx or 5xx status and the body `{"error": "message"}`.

Paths are always absolute; clients compute relative paths themselves. `size` is in bytes and `modified` is Unix seconds.

## Endpoints

### `GET /v1/status`

```json
{
  "app": "fffinder", "version": "0.2.0", "protocol": 1, "pid": 4242,
  "watch": true, "scanning": false, "finishing": false, "files": 672643, "dirs": 91420,
  "roots": [
    {"path": "/Users/me", "scanning": false, "finishing": false, "files": 264824, "dirs": 40122, "git_repo": false}
  ]
}
```

`scanning` is true while a location's index is being built and its results are incomplete. `finishing` is true after the scan, while fff-search still reads each file once to flag binaries; searches are complete then, but the CPU is busy.

### `GET /v1/find`

Fuzzy search by name, ranked the way the window ranks results.

| Parameter | Meaning |
|---|---|
| `query` | Search text, with fffinder's search syntax. Required. |
| `type` | `all` (default), `file`, or `dir`. |
| `kind` | `document`, `image`, `movie`, `audio`, `code`, `archive`, or `folder`. |
| `scope` | Only results inside this folder. |
| `limit` | Maximum rows (default 100). |

Rows: `{"type": "file", "path": "…", "rank": 1, "score": 812, "size": 2048, "modified": 1760000000, "git": "modified"}`. `type` is always `file` or `dir`. `size` and `modified` are omitted for folders; `git` only when the file has a git status.

### `GET /v1/files` and `GET /v1/dirs`

Every indexed file (or folder), or those matching `query` when given. `limit` caps the rows; there is no default cap.

Files: `{"path": "…", "root": "…", "size": 2048, "modified": 1760000000}`. Folders: `{"path": "…", "root": "…"}`. `root` is the search location the item belongs to. Folder paths have no trailing separator, and the search locations themselves are not listed. On macOS, with `search_apps` on, `/v1/dirs` also lists the apps and the folders that hold them in the app folders, which aren't indexed; their `root` is the app folder, such as `/Applications`.

### `GET /v1/search`

Search file contents.

| Parameter | Meaning |
|---|---|
| `pattern` | Text to find. Repeat for several patterns; a line matching any of them counts. A glob in a pattern (`*.rs`, `src/**`) limits the files searched, as in `idx search`. Required. |
| `mode` | `plain` (default), `regex`, or `fuzzy`. |
| `before`, `after` | Context lines around each match (default 0). |
| `scope` | Only files inside this folder. |
| `limit` | Maximum matches (default 200; clients usually send their own). |

Rows: `{"path": "…", "line_number": 12, "column": 4, "byte_offset": 310, "line": "…", "matches": [{"start": 314, "end": 318}], "context_before": [], "context_after": []}`.

A `scope` written through a symlink (`/private/var/…` for `/var/…` on macOS) is matched to the search location it resolves into, here and in `/v1/find`. `matches` are byte offsets from the start of the file, like `idx search`.

### `GET /v1/watch`

Stream file changes until the client disconnects.

| Parameter | Meaning |
|---|---|
| `pattern` | Glob relative to each search location, an absolute path, or empty for everything. |
| `ignore` | Glob or path prefix to skip. Repeatable. |
| `timeout_ms` | Stop after this long. |
| `max_events` | Stop after this many events. |

Lines: `{"kind": "modified", "path": "…"}`, plus `"from"` for `renamed`. Kinds are `created`, `modified`, `removed`, `renamed`, and `rescan`.

### `GET /v1/selection`

What the frontmost window shows. With `all=true`, `items` holds every item shown instead of only the selected ones.

```json
{
  "location": "folder", "folder": "/Users/me/src", "query": null,
  "items": [{"path": "…", "name": "README.md", "type": "file", "size": 1200, "modified": 1760000000}]
}
```

`location` is `folder`, `recents`, `search`, or `results`.

### `POST /v1/show`

Show these files in the frontmost window, or in a new window if none is open.

Body: `{"paths": ["…"], "title": "Large images"}`. Reply: `{"shown": 12, "missing": ["…"]}`.

### `POST /v1/open`

Bring a window forward, at a folder and with a search when given.

Body: `{"path": "/Users/me/src", "query": "readme"}`, both optional. A file path opens its folder with the file selected. Reply: `{"ok": true}`.

## Notes for clients

- Results use the search locations' own spelling of paths. A client whose working folder is spelled through a symlink (`/private/var/…` for `/var/…`) should resolve both before computing relative paths.
- `renamed` watch events carry `from`; other kinds don't.
