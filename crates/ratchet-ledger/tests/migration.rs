//! The Rust migration must create the same schema as the drizzle migration it was ported from
//! (`packages/core/src/ledger/migrations/0000_init.sql`), the executable spec until the TypeScript is deleted.

mod common;

use std::path::PathBuf;

use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{Row, SqlitePool};

async fn schema(pool: &SqlitePool) -> Vec<(String, String, String, Option<String>)> {
    sqlx::query(
        "select type, name, tbl_name, sql from sqlite_master \
         where name not like 'sqlite_%' and name not like '\\_sqlx%' escape '\\' order by type, name",
    )
    .fetch_all(pool)
    .await
    .unwrap()
    .iter()
    .map(|r| (r.get("type"), r.get("name"), r.get("tbl_name"), r.get("sql")))
    .collect()
}

#[tokio::test]
async fn the_migration_creates_the_same_schema_as_the_drizzle_one() {
    let l = common::ledger().await;
    let ours = schema(l.pool()).await;

    let drizzle = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../packages/core/src/ledger/migrations/0000_init.sql"),
    )
    .unwrap();
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    for statement in drizzle.split("--> statement-breakpoint") {
        if statement.trim().is_empty() {
            continue;
        }
        sqlx::raw_sql(sqlx::AssertSqlSafe(statement.to_owned()))
            .execute(&pool)
            .await
            .unwrap();
    }
    let theirs = schema(&pool).await;

    assert_eq!(ours.len(), theirs.len(), "{ours:#?}\n{theirs:#?}");
    for (a, b) in ours.iter().zip(&theirs) {
        assert_eq!(a.0, b.0, "type");
        assert_eq!(a.1, b.1, "name");
        assert_eq!(a.2, b.2, "table of {}", a.1);
        // SQLite keeps the statement text as written
        assert_eq!(
            a.3.as_deref().map(str::trim),
            b.3.as_deref().map(str::trim),
            "DDL of {}",
            a.1
        );
    }
    // 10 tables, 10 indexes (the primary-key autoindexes included), 4 triggers
    let count = |t: &str| ours.iter().filter(|r| r.0 == t).count();
    assert_eq!(
        (count("table"), count("index"), count("trigger")),
        (10, 10, 4)
    );
}

#[tokio::test]
async fn opening_twice_applies_the_migration_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    for _ in 0..2 {
        let l = ratchet_ledger::Ledger::open(&path, Default::default())
            .await
            .unwrap();
        let n: i64 = sqlx::query("select count(*) n from _sqlx_migrations")
            .fetch_one(l.pool())
            .await
            .unwrap()
            .get("n");
        assert_eq!(n, 1);
        l.close().await;
    }
}
