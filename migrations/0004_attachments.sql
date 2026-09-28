-- Uploaded images attached to issue/comment markdown. Bytes live under
-- <data_dir>/attachments/<project_id>/<filename>; this table is the authorization/metadata index.
CREATE TABLE attachments (
  id INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  filename TEXT NOT NULL UNIQUE,
  content_type TEXT NOT NULL,
  byte_size INTEGER NOT NULL,
  created_by_kind TEXT NOT NULL,
  created_by_name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX attachments_project ON attachments(project_id);
