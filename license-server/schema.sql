PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS license_keys (
  id TEXT PRIMARY KEY,
  key_hash TEXT NOT NULL UNIQUE,
  status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'revoked')),
  plan TEXT NOT NULL DEFAULT 'lifetime',
  max_devices INTEGER NOT NULL DEFAULT 1 CHECK (max_devices = 1),
  expires_at TEXT,
  note TEXT,
  created_at TEXT NOT NULL,
  revoked_at TEXT
);

CREATE TABLE IF NOT EXISTS device_activations (
  id TEXT PRIMARY KEY,
  license_key_id TEXT NOT NULL,
  device_hash TEXT NOT NULL,
  platform TEXT,
  app_version TEXT,
  first_activated_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  FOREIGN KEY (license_key_id) REFERENCES license_keys(id) ON DELETE CASCADE,
  UNIQUE (license_key_id, device_hash)
);

CREATE INDEX IF NOT EXISTS idx_license_keys_status
  ON license_keys(status);

CREATE INDEX IF NOT EXISTS idx_device_activations_license_key_id
  ON device_activations(license_key_id);
