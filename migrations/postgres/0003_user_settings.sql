CREATE TABLE user_settings (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    light_mode BOOLEAN NOT NULL DEFAULT false
);
