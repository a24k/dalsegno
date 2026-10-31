# Data model

SQLite in WAL mode, with `foreign_keys = ON`. Migrations are embedded SQL files applied
in order, tracked by `PRAGMA user_version`.

Until milestone 5 starts the writer thread, the connection is shared behind a mutex.

Conventions: `stream_time` is seconds of stream clock (`REAL`), always paired with a
`stream_id` (D-10). `*_at` is epoch milliseconds UTC. JST is applied only for display.

## Schema

```sql
CREATE TABLE programs (
  id INTEGER PRIMARY KEY,
  short_name TEXT NOT NULL,
  channel_id TEXT,
  name_template TEXT NOT NULL,
  addition_suffix TEXT NOT NULL,
  artist TEXT, content_date TEXT, description TEXT,
  output_dir TEXT NOT NULL
);

CREATE TABLE streams (
  id INTEGER PRIMARY KEY,
  video_id TEXT NOT NULL UNIQUE,
  program_id INTEGER REFERENCES programs(id),
  title TEXT NOT NULL,
  channel_id TEXT, channel_name TEXT, description TEXT, thumbnail_url TEXT,
  state TEXT NOT NULL CHECK (state IN
    ('registered','backfilling','recording','paused','error','ended')),
  state_reason TEXT,
  video_itag INTEGER, audio_itag INTEGER NOT NULL,
  fragment_len REAL NOT NULL,
  follow_sq INTEGER,                  -- next sq at the head; never behind a gap
  period REAL,                        -- NULL means no loop detected yet
  testable_period_max REAL,
  registered_at INTEGER NOT NULL,
  last_fetch_at INTEGER
);

CREATE TABLE fragments (
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  itag INTEGER NOT NULL,
  sq INTEGER NOT NULL,
  stream_time REAL NOT NULL,
  duration REAL NOT NULL,
  size INTEGER NOT NULL,
  fetched_at INTEGER NOT NULL,
  PRIMARY KEY (stream_id, itag, sq)
) WITHOUT ROWID;

CREATE TABLE gaps (
  id INTEGER PRIMARY KEY,
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  itag INTEGER NOT NULL,
  sq_first INTEGER NOT NULL, sq_last INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('open','filled','lost'))
);

CREATE TABLE envelope (
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  sq INTEGER NOT NULL,
  first_bin INTEGER NOT NULL,         -- grid index of the first value; bin n starts at n / rate
  feature TEXT NOT NULL DEFAULT 'env10',
  data BLOB NOT NULL,                 -- little-endian f32
  PRIMARY KEY (stream_id, feature, sq)
) WITHOUT ROWID;

CREATE TABLE feature_runs (
  id INTEGER PRIMARY KEY,
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  feature TEXT NOT NULL,
  sq_first INTEGER NOT NULL,
  next_sq INTEGER NOT NULL,
  anchor_time REAL NOT NULL           -- stream time of the run's first sample
);

CREATE TABLE retention_holds (
  id INTEGER PRIMARY KEY,
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  itag INTEGER NOT NULL,
  sq_first INTEGER NOT NULL, sq_last INTEGER NOT NULL,
  version_id INTEGER REFERENCES versions(id),
  change_id INTEGER REFERENCES changes(id),
  clip_id INTEGER REFERENCES clips(id),
  expires_at INTEGER,                 -- NULL while the owner is open
  CHECK ((version_id IS NOT NULL) + (change_id IS NOT NULL) + (clip_id IS NOT NULL) = 1)
);
CREATE INDEX retention_holds_range ON retention_holds (stream_id, itag, sq_first, sq_last);

CREATE TABLE head_samples (
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  observed_at INTEGER NOT NULL,
  head_sq INTEGER NOT NULL,
  head_time REAL NOT NULL,
  PRIMARY KEY (stream_id, observed_at)
) WITHOUT ROWID;

CREATE TABLE versions (
  id INTEGER PRIMARY KEY,
  program_id INTEGER NOT NULL REFERENCES programs(id),
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  period_audio REAL NOT NULL,
  period_video REAL,
  anchor_time REAL NOT NULL,          -- start of the held cycle
  anchor_at INTEGER NOT NULL,
  first_seen_at INTEGER NOT NULL,
  on_air_from_time REAL NOT NULL,
  on_air_until_time REAL,             -- set when superseded; FR-E5 checks against this range
  last_cycle_start_time REAL          -- set when superseded; drives the DVR deadline
);

CREATE TABLE analysis_state (
  stream_id INTEGER PRIMARY KEY REFERENCES streams(id),
  tracked_until REAL NOT NULL,        -- stream time of the last tracking point
  search_widened INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE lag_runs (
  id INTEGER PRIMARY KEY,
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  start_time REAL NOT NULL, end_time REAL NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('lag','outlier','unmatched','no_data')),
  lag REAL
);

CREATE TABLE changes (
  id INTEGER PRIMARY KEY,
  program_id INTEGER NOT NULL REFERENCES programs(id),
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  detected_at INTEGER NOT NULL,
  old_version_id INTEGER REFERENCES versions(id),
  new_version_id INTEGER REFERENCES versions(id),
  state TEXT NOT NULL CHECK (state IN
    ('provisional','confirmed','resolved','dismissed')),
  dvr_deadline_at INTEGER,
  checks TEXT                         -- JSON array of SelfCheck
);
CREATE UNIQUE INDEX changes_one_provisional ON changes (program_id)
  WHERE state = 'provisional';

CREATE TABLE notifications (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  subject TEXT NOT NULL,              -- for example change:12:confirmed
  body TEXT NOT NULL,                 -- JSON
  created_at INTEGER NOT NULL,
  sent_at INTEGER,
  attempts INTEGER NOT NULL DEFAULT 0,
  UNIQUE (kind, subject)
);

CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL                 -- JSON; operational keys only (D-24)
);

CREATE TABLE change_regions (
  id INTEGER PRIMARY KEY,
  change_id INTEGER NOT NULL REFERENCES changes(id),
  class TEXT NOT NULL CHECK (class IN
    ('existing','new','revival','known','deleted','no_data')),
  start_time REAL NOT NULL, end_time REAL NOT NULL,
  match_version_id INTEGER REFERENCES versions(id),
  match_start_time REAL
);

CREATE TABLE clips (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL UNIQUE,
  kind TEXT NOT NULL CHECK (kind IN ('cycle','addition')),
  version_id INTEGER REFERENCES versions(id),
  change_id INTEGER REFERENCES changes(id),
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  video_itag INTEGER NOT NULL, audio_itag INTEGER NOT NULL,
  sq_first INTEGER NOT NULL, sq_last INTEGER NOT NULL,
  duration REAL NOT NULL,
  metadata TEXT NOT NULL,             -- JSON, as written to the mp4
  poster_time REAL,
  output_path TEXT,
  plex_status TEXT NOT NULL DEFAULT 'none'
    CHECK (plex_status IN ('none','requested','confirmed','failed'))
);

CREATE TABLE known_clips (
  id INTEGER PRIMARY KEY,
  program_id INTEGER NOT NULL REFERENCES programs(id),
  name TEXT NOT NULL,
  duration REAL NOT NULL,
  feature TEXT NOT NULL,
  data BLOB NOT NULL
);

CREATE TABLE jobs (
  id INTEGER PRIMARY KEY,
  kind TEXT NOT NULL,
  class TEXT NOT NULL,
  target TEXT NOT NULL,               -- JSON
  state TEXT NOT NULL CHECK (state IN
    ('queued','running','succeeded','failed','cancelled')),
  progress REAL NOT NULL DEFAULT 0,
  checkpoint TEXT,                    -- JSON
  error TEXT,
  created_at INTEGER NOT NULL, started_at INTEGER, finished_at INTEGER
);
CREATE INDEX jobs_claim ON jobs (state, class, id);

CREATE TABLE ip_family_events (
  id INTEGER PRIMARY KEY,
  at INTEGER NOT NULL, family TEXT NOT NULL, reason TEXT NOT NULL
);
```

## Time mapping

`core::timemap` has no formula. Lookups interpolate between the nearest measurements:

- `sq` to stream time: `fragments`, falling back to `head_samples` for `sq` not yet stored.
- Stream time to wall clock: `head_samples`.

Interpolating across more than 10 minutes is flagged as approximate in results.

## Retention

| Data | Rule |
| --- | --- |
| Audio files | 14 days from fetch, unless covered by a live hold (D-11, D-12). |
| Video files | No base retention: kept only while covered by a live hold. |
| Holds | A version's hold never expires. Others last 7 days past the owner's close. |
| `fragments` rows | Kept with their files. |
| Envelope | Same as audio. |
| `lag_runs` | Same as audio. |
| Notifications | Sent notifications 30 days. |
| `head_samples` | 30 days, then thinned to hourly. |
| Jobs | Succeeded jobs 30 days; failed jobs are kept. |

File paths derive from the fragment key, so they are not stored. The sweep runs daily and
removes the file before the row, so a crash leaves a row whose file is missing, which is
detected and repaired by the next sweep.

## Size estimate

For one stream at 5-second fragments: 17,280 rows a day per table and 3.5 MB of envelope
a day, so under 10 MB a day in total, which is comfortable for SQLite.
