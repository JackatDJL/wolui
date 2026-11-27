CREATE TABLE IF NOT EXISTS connect_sites (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  token_digest TEXT UNIQUE,
  is_local INTEGER NOT NULL DEFAULT 0,
  active INTEGER NOT NULL DEFAULT 1,
  last_seen_at INTEGER,
  created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS connect_invitations (
  token_digest TEXT PRIMARY KEY,
  site_name TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  used_at INTEGER
);

CREATE TABLE IF NOT EXISTS remote_devices (
  site_id TEXT NOT NULL REFERENCES connect_sites(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  name TEXT NOT NULL,
  mac TEXT NOT NULL,
  network_profile_id TEXT NOT NULL,
  network_profile_name TEXT NOT NULL,
  tailscale_name TEXT,
  revision INTEGER NOT NULL,
  PRIMARY KEY(site_id, id)
);

CREATE TABLE IF NOT EXISTS connect_commands (
  id TEXT PRIMARY KEY,
  site_id TEXT NOT NULL REFERENCES connect_sites(id) ON DELETE CASCADE,
  device_id TEXT NOT NULL,
  idempotency_key TEXT NOT NULL,
  status TEXT NOT NULL,
  expires_at INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE(site_id, idempotency_key)
);

INSERT OR IGNORE INTO connect_sites(id, name, is_local, active, created_at)
SELECT value, 'Local site', 1, 1, CAST(strftime('%s','now') AS INTEGER)
FROM settings WHERE key = 'site_id';
