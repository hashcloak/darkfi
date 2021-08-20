CREATE TABLE IF NOT EXISTS keys(
    key_id INTEGER PRIMARY KEY NOT NULL,
    key_public BLOB NOT NULL,
    key_private BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS keypairs(
    dkey_id INTEGER PRIMARY KEY NOT NULL,
    btc_key_private BLOB NOT NULL,
    btc_key_public BLOB NOT NULL,
    txid BLOB
);
CREATE TABLE IF NOT EXISTS withdraw_keypairs(
    btc_key_id INTEGER PRIMARY KEY NOT NULL,
    d_key_public BLOB NOT NULL,
    txid BLOB
);