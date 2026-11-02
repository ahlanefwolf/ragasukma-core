-- 0002_chat_history.sql
-- Create table for storing conversation turns and chat history in SQLite

CREATE TABLE IF NOT EXISTS chat_history (
    id TEXT PRIMARY KEY NOT NULL,
    timestamp INTEGER NOT NULL,
    date_str TEXT NOT NULL,
    time_str TEXT NOT NULL,
    callsign TEXT NOT NULL,
    assistant_name TEXT NOT NULL,
    role TEXT NOT NULL,
    user_msg TEXT NOT NULL,
    assistant_msg TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_chat_history_timestamp ON chat_history(timestamp DESC);
CREATE INDEX IF NOT EXISTS idx_chat_history_date ON chat_history(date_str);
