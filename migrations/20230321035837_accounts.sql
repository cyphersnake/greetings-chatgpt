CREATE TABLE IF NOT EXISTS "api_keys"
(
    "key_hash"   BLOB(32) NOT NULL UNIQUE,
    "key_prefix" BLOB(10) NOT NULL UNIQUE PRIMARY KEY
);

CREATE TABLE IF NOT EXISTS "users"
(
    "chat_id"        INTEGER  NOT NULL PRIMARY KEY,
    "current_prompt" TEXT,
    "history"        TEXT NOT NULL, -- Store history as a serialized JSON
    "api_key_prefix" BLOB(10) NOT NULL,

    FOREIGN KEY ("api_key_prefix") REFERENCES "api_keys" ("key_prefix") ON DELETE CASCADE
);

