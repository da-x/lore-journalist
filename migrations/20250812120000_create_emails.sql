-- Initial emails table (mailing-list corpus from build-db).
-- IF NOT EXISTS allows applying this migration to DBs that already have the table
-- from the pre-migration CREATE TABLE path.

CREATE TABLE IF NOT EXISTS emails (
    message_id TEXT PRIMARY KEY NOT NULL,
    subject TEXT NOT NULL,
    from_addr TEXT NOT NULL,
    date TEXT NOT NULL,
    body BLOB NOT NULL,
    in_reply_to TEXT,
    "references" TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_emails_date ON emails (date);
