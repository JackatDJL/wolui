PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS sessions (
  token_digest TEXT PRIMARY KEY,
  csrf_token TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS network_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  interface TEXT NOT NULL,
  bind_address TEXT NOT NULL,
  broadcast_address TEXT NOT NULL,
  port INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS devices (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  mac TEXT NOT NULL,
  network_profile_id TEXT NOT NULL REFERENCES network_profiles(id),
  tailscale_name TEXT,
  revision INTEGER NOT NULL DEFAULT 1,
  UNIQUE(network_profile_id, mac)
);
CREATE TABLE IF NOT EXISTS wake_jobs (
  id TEXT PRIMARY KEY,
  idempotency_key TEXT NOT NULL UNIQUE,
  device_id TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
