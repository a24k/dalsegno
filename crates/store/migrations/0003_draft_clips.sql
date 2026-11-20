-- A clip is a draft from the moment regions are selected until it is exported, under one ID.
-- SQLite cannot drop NOT NULL, so the table is rebuilt.
CREATE TABLE clips_new (
  id INTEGER PRIMARY KEY,
  state TEXT NOT NULL DEFAULT 'exported' CHECK (state IN ('draft','exported')),
  name TEXT UNIQUE,                   -- set at export
  kind TEXT NOT NULL CHECK (kind IN ('cycle','addition')),
  version_id INTEGER REFERENCES versions(id),
  change_id INTEGER REFERENCES changes(id),
  stream_id INTEGER NOT NULL REFERENCES streams(id),
  start_time REAL,                    -- chosen cut, in stream time
  end_time REAL,
  video_itag INTEGER, audio_itag INTEGER,
  sq_first INTEGER, sq_last INTEGER,
  duration REAL,
  metadata TEXT,                      -- JSON, as written to the mp4
  poster_time REAL,
  output_path TEXT,
  plex_status TEXT NOT NULL DEFAULT 'none'
    CHECK (plex_status IN ('none','requested','confirmed','failed')),
  CHECK (state = 'draft' OR (name IS NOT NULL AND video_itag IS NOT NULL
    AND audio_itag IS NOT NULL AND sq_first IS NOT NULL AND sq_last IS NOT NULL
    AND duration IS NOT NULL AND metadata IS NOT NULL)),
  CHECK (state = 'exported' OR (start_time IS NOT NULL AND end_time IS NOT NULL))
);
INSERT INTO clips_new (id, state, name, kind, version_id, change_id, stream_id, video_itag,
  audio_itag, sq_first, sq_last, duration, metadata, poster_time, output_path, plex_status)
  SELECT id, 'exported', name, kind, version_id, change_id, stream_id, video_itag, audio_itag,
  sq_first, sq_last, duration, metadata, poster_time, output_path, plex_status FROM clips;
DROP TABLE clips;
ALTER TABLE clips_new RENAME TO clips;
CREATE INDEX clips_change ON clips (change_id);
