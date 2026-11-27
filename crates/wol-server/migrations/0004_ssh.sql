ALTER TABLE devices ADD COLUMN ssh_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE devices ADD COLUMN ssh_username TEXT;

ALTER TABLE remote_devices ADD COLUMN ssh_enabled INTEGER NOT NULL DEFAULT 0;
ALTER TABLE remote_devices ADD COLUMN ssh_username TEXT;

CREATE TABLE ssh_sessions (
  id TEXT PRIMARY KEY,
  site_id TEXT NOT NULL,
  device_id TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  outcome TEXT NOT NULL
);

CREATE INDEX ssh_sessions_started_at ON ssh_sessions(started_at);
