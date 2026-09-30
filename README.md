# photos-migrator

A fast, self-contained CLI utility written in Rust to migrate Google Photos libraries and albums between Google accounts (supporting `@gmail.com` and Google Workspace accounts), working with a local Google Takeout archive.

---

## Key Features

1. **Local Takeout Processing & In-Place Metadata Injection**:
   - Parses Google Takeout directory trees and sidecar `.json` files.
   - Pairs truncated filenames (Google truncates sidecars at ~47-51 characters), numbered files (`photo(1).jpg`), and edited versions (`photo-edited.jpg`).
   - Injects original capture dates, GPS coordinates, and camera info into image headers (EXIF/XMP).
   - In-place modification by default to conserve disk space, with an optional `--output-dir` flag.
2. **"Known Faces" & People Preservation**:
   - Extracts tagged people from Takeout JSON sidecars.
   - Embeds them into standard XMP/IPTC tags (`XMP-iptcExt:PersonInImage`, `XMP:Subject`, `IPTC:Keywords`).
   - Appends tagged people to the photo description (`Description: ... | People: Alice, Bob`) so they are immediately searchable and visible in the Google Photos web and mobile UI.
3. **Album Recreation & Sharing**:
   - Recreates albums on the target Google Photos account via the Google Photos Library API.
   - Deduplicates photos using SHA-256 hashes to prevent redundant uploads across year and album folders.
   - Enables link sharing (`albums.share`) to obtain joinable URLs.
   - Matches albums with original collaborator names and generates an Invitation Report (`invitation_report.md`).
4. **Crash-Proof Pause & Resume**:
   - Uses an embedded zero-dependency SQLite database (`.photos-migrator.db`) to record all progress, file hashes, Google Media IDs, and album mappings.
   - Pause or interrupt anytime; running `upload` resumes exactly where it left off.
5. **Single Self-Contained Binary**:
   - Statically linked SQLite (`rusqlite` bundled) and pure-Rust TLS (`rustls`). No OpenSSL or runtime dependencies needed.
   - Pure Rust metadata manipulation for JPEG, PNG, and WebP, with automatic detection and seamless fallback to `exiftool` if installed for complex video (MP4/MOV) and Apple HEIC tags.

---

## Quick Start

### 1. Build the Binary
```bash
cargo build --release
```
The compiled binary will be located at `target/release/photos-migrator`.

### 2. Google Cloud OAuth Setup (2 minutes)
To interact with Google Photos, you need OAuth credentials with your dedicated daily quota (10,000 requests/day):
1. Go to [Google Cloud Console](https://console.cloud.google.com).
2. Create a project (e.g. `Photos Migration`) and enable the **Photos Library API**.
3. Under **OAuth consent screen**, set User Type to **External**, add your Google accounts as **Test users**, and add the scopes `https://www.googleapis.com/auth/photoslibrary` and `https://www.googleapis.com/auth/photoslibrary.sharing`.
4. Under **Credentials**, click **Create Credentials** $\to$ **OAuth client ID** $\to$ select **Desktop app**.
5. Download the `credentials.json` file or copy the Client ID & Secret.

Authenticate with `photos-migrator`:
```bash
./target/release/photos-migrator auth --credentials /path/to/credentials.json
```
*(Or run `photos-migrator auth` without flags to enter Client ID and Secret interactively. A browser window will open to complete the login.)*

---

## Command Reference

### `photos-migrator migrate <TAKEOUT_DIR>`
All-in-one guided wizard that steps through authentication, metadata fixing, planning, uploading, and sharing report.

### `photos-migrator auth`
Configures Google Cloud OAuth credentials and logs into the target Google account.
```bash
photos-migrator auth [--account target] [--credentials path/to/credentials.json]
```

### `photos-migrator fix-metadata <TAKEOUT_DIR>`
Scans the Takeout archive offline and embeds timestamps, GPS, and face tags directly into media files.
```bash
# In-place modification (saves disk space)
photos-migrator fix-metadata /path/to/Takeout

# Export to a separate directory instead
photos-migrator fix-metadata /path/to/Takeout --output-dir /path/to/ProcessedPhotos

# Dry-run preview
photos-migrator fix-metadata /path/to/Takeout --dry-run
```

### `photos-migrator plan <TAKEOUT_DIR>`
Analyzes the archive, computes SHA-256 hashes, deduplicates files across album and year folders, and previews the migration plan:
```bash
photos-migrator plan /path/to/Takeout
```

### `photos-migrator upload <TAKEOUT_DIR>`
Uploads pending media items in parallel, registers them in batches of 50, recreates albums, and links items:
```bash
photos-migrator upload /path/to/Takeout --concurrency 4
```

### `photos-migrator share-report`
Displays album status and outputs `invitation_report.md` containing album names, original collaborator names, and joinable Google Photos URLs:
```bash
photos-migrator share-report [--output invitation_report.md]
```

---

## Database & State Tracking
All state is stored in an embedded SQLite database named `.photos-migrator.db` in the working directory (customizable via `--db <path>`). This stores:
- `files`: SHA-256 hash, file path, upload token, Google Media ID, upload timestamp.
- `albums`: Album name, source collaborators, Google Album ID, share token, shareable URL.
- `album_items`: Many-to-many relationship between albums and photo hashes.
