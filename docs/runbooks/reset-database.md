# Reset the database

**When to use:** dev iteration on schema changes; recovering from
a corrupted database; starting from scratch after a botched migration.

## Command

```bash
cd backend
just reset
```

This:
- Deletes `data/kino.db` (+ `-wal` and `-shm` companions).
- Clears the librqbit session state (`data/librqbit/session.json`
  and torrent files registered with it).
- Restarts the `kino-backend` container so the binary reads a
  fresh database.

The new DB picks up every migration from scratch. The setup
wizard appears on next browser visit because `config.api_key`
hasn't been written yet.

## What's preserved

- The `data/library/` and `data/downloads/` directories on disk.
  Files are not touched. After reset, kino has no record of them
  but they're still there. To re-import: re-follow the show/movie
  + manually link, or wait for the orphan-file invariant to
  surface them.
- The `data/definitions/` cardigann YAMLs. Reset doesn't re-pull;
  the existing set is reused.

## What's lost

Everything in the database:

- All movies, shows, episodes, media records.
- All download history.
- All blocklist entries.
- All session cookies. Same-machine browsers auto-recover via the
  loopback `auto-localhost` session path; remote devices re-paste
  the API key (or pair via QR from a reauthed device).
- All preferences, quality profiles, indexers.
- Trakt OAuth tokens (need to reconnect).

## Don't do this in production

`just reset` is a dev-only tool. There is no equivalent
production "reset" command — for good reason. The binary's `kino
reset` subcommand still exists if you really mean it, but it
takes the same data path and is just as destructive.
