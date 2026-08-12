# SQLx setup

This crate uses **SQLx compile-time checked queries** (`sqlx::query!`) and **embedded migrations**.

## Layout

| Path | Role |
|---|---|
| `migrations/` | Versioned SQL migrations (applied at runtime via `sqlx::migrate!`) |
| `.sqlx/` | Offline query metadata for `query!` macros (committed) |
| `.cargo/config.toml` | Sets `SQLX_OFFLINE=true` so builds do not need a live DB |

## Runtime

`crate::db::open_db` opens the SQLite file and runs pending migrations before returning a pool. `build-db`, `meta`, and `grep` all go through this path.

## After changing SQL or migrations

Refresh offline data so `query!` macros keep working offline:

```bash
# From crate root
rm -f target/sqlx-prepare.db
export DATABASE_URL=sqlite:target/sqlx-prepare.db
sqlx database create
sqlx migrate run
SQLX_OFFLINE=false cargo sqlx prepare --database-url "$DATABASE_URL" -- --all-targets
```

Commit the updated `.sqlx/` files.

Requires `cargo install sqlx-cli --features sqlite` (CLI major version should be compatible with the `sqlx` crate in `Cargo.toml`).

## Adding a migration

```bash
sqlx migrate add describe_change
# edit migrations/*_describe_change.sql
# then re-run prepare as above
```
