ALTER TABLE users ADD COLUMN referal INTEGER REFERENCES users (id);
ALTER TABLE users ADD COLUMN utm_marks jsonb;
ALTER TABLE users ADD COLUMN country VARCHAR;
ALTER TABLE users ADD COLUMN referer VARCHAR;
