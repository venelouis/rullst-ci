//! REVIEW-ONLY (PR #754 review, not for merge): the jiff-based types against
//! SQLx's chrono codecs on live servers, through both the binary (prepared)
//! and the text (simple query) protocols, which the ORM's own contracts do
//! not reach. Skips a server whose `REVIEW_*_URL` is unset.
#![cfg(feature = "drivers-all")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fmt::Debug;
use std::panic::{AssertUnwindSafe, catch_unwind};

use rullst_orm::chrono::{
    DateTime as CDateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc,
};
use rullst_orm::jiff::{self, civil};
use rullst_orm::sqlx::{self, Connection, Decode, Row, Type};
use rullst_orm::{Date, DateTime, Time, Timestamp};

fn j_time(t: NaiveTime) -> civil::Time {
    civil::Time::new(
        t.hour() as i8,
        t.minute() as i8,
        t.second() as i8,
        (t.nanosecond() % 1_000_000_000) as i32,
    )
    .unwrap()
}
fn j_date(d: NaiveDate) -> Option<civil::Date> {
    civil::Date::new(i16::try_from(d.year()).ok()?, d.month() as i8, d.day() as i8).ok()
}
fn j_dt(n: NaiveDateTime) -> Option<civil::DateTime> {
    Some(j_date(n.date())?.to_datetime(j_time(n.time())))
}
fn j_ts(c: CDateTime<Utc>) -> Option<jiff::Timestamp> {
    jiff::tz::Offset::UTC.to_timestamp(j_dt(c.naive_utc())?).ok()
}

/// "ok:<value>", "err" or "PANIC".
fn outcome<T: Debug>(r: std::thread::Result<Result<Option<T>, sqlx::Error>>) -> String {
    match r {
        Ok(Ok(Some(v))) => format!("ok:{v:?}"),
        Ok(Ok(None)) => "ok:<outside jiff range>".to_string(),
        Ok(Err(_)) => "err".to_string(),
        Err(_) => "PANIC".to_string(),
    }
}

fn get<'r, R, T>(row: &'r R, column: &str) -> std::thread::Result<Result<T, sqlx::Error>>
where
    R: Row,
    T: Decode<'r, R::Database> + Type<R::Database>,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
{
    catch_unwind(AssertUnwindSafe(|| row.try_get::<T, _>(column)))
}

/// Compares our decode of `column` with chrono's; `None` when they agree.
fn compare<'r, R, O, C, V>(
    row: &'r R,
    column: &str,
    ours: impl Fn(O) -> V,
    theirs: impl Fn(C) -> Option<V>,
) -> (String, String, bool)
where
    R: Row,
    O: Decode<'r, R::Database> + Type<R::Database>,
    C: Decode<'r, R::Database> + Type<R::Database>,
    V: Debug + PartialEq,
    for<'c> &'c str: sqlx::ColumnIndex<R>,
{
    let o = get::<R, O>(row, column).map(|r| r.map(|v| Some(ours(v))));
    let c = get::<R, C>(row, column).map(|r| r.map(&theirs));
    let agree = match (&o, &c) {
        (Ok(Ok(Some(a))), Ok(Ok(Some(b)))) => a == b,
        (Ok(Err(_)), Ok(Err(_)) | Ok(Ok(None)) | Err(_)) => true,
        _ => false,
    };
    (outcome(o), outcome(c), agree)
}

fn report<R: Row>(label: &str, row: &R, checks: Vec<(&str, (String, String, bool))>, bad: &mut Vec<String>) {
    let _ = row;
    for (what, (ours, theirs, agree)) in checks {
        let mark = if agree { "  " } else { "!!" };
        let line = format!("{mark} {label} {what}: ours={ours} chrono={theirs}");
        println!("{line}");
        if !agree {
            bad.push(line);
        }
    }
}

#[tokio::test]
async fn review_postgres_binary_and_text_reads_match_chrono() {
    let Ok(url) = std::env::var("REVIEW_PG_URL") else {
        eprintln!("REVIEW_PG_URL unset: skipped");
        return;
    };
    let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
    sqlx::raw_sql(
        "CREATE TEMP TABLE t (id int, tz timestamptz, ts timestamp, d date, tm time);
         INSERT INTO t VALUES
          (1, '2026-10-15 09:30:00.5+00', '2026-10-15 09:30:00.5', '2026-10-15', '09:30:00.5'),
          (2, '1999-12-31 23:59:59.999999+00', '1999-12-31 23:59:59.999999', '1999-12-31', '23:59:59.999999'),
          (3, '0001-01-01 00:00:00+00', '0001-01-01 00:00:00', '0001-01-01', '00:00:00'),
          (4, '0044-03-15 12:00:00+00 BC', '0044-03-15 12:00:00 BC', '0044-03-15 BC', '24:00:00'),
          (5, 'infinity', '-infinity', 'infinity', '12:00'),
          (6, '9999-12-31 23:59:59.999999+00', '9999-12-31 23:59:59.999999', '9999-12-31', '00:00:00.000001'),
          (7, '10000-01-01 00:00:00+00', '10000-01-01 00:00:00', '10000-01-01', '00:00:01'),
          (8, '1900-01-01 00:00:00+00', '1900-01-01 00:00:00', '1900-01-01', '06:00'),
          (9, '9999-12-30 22:00:00+00', '-4713-11-24 00:00:00', '4714-11-24 BC', '23:59:59'),
          (10, '9999-12-31 12:00:00+00', '2040-02-29 23:59:59.5', '2040-02-29', '00:00:00.5');",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let mut bad = Vec::new();
    for zone in ["UTC", "America/Sao_Paulo", "Pacific/Chatham", "America/St_Johns", "Europe/Amsterdam"] {
        sqlx::raw_sql(&format!("SET TIME ZONE '{zone}'"))
            .execute(&mut conn)
            .await
            .unwrap();
        let binary = sqlx::query("SELECT id, tz, ts, d, tm, tz::text AS tz_text FROM t ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .unwrap();
        let text = sqlx::raw_sql("SELECT id, tz, ts, d, tm, tz::text AS tz_text FROM t ORDER BY id")
            .fetch_all(&mut conn)
            .await
            .unwrap();
        for (protocol, rows) in [("binary", &binary), ("text", &text)] {
            for row in rows.iter() {
                let id: i32 = row.get("id");
                let shown: String = row.get("tz_text");
                let label = format!("pg {zone} {protocol} row {id} ({shown})");
                let checks = vec![
                    ("tz", compare(row, "tz", |v: Timestamp| *v, |c: CDateTime<Utc>| j_ts(c))),
                    ("ts", compare(row, "ts", |v: DateTime| *v, |c: NaiveDateTime| j_dt(c))),
                    ("d", compare(row, "d", |v: Date| *v, |c: NaiveDate| j_date(c))),
                    ("tm", compare(row, "tm", |v: Time| *v, |c: NaiveTime| Some(j_time(c)))),
                ];
                report(&label, row, checks, &mut bad);
            }
        }
    }
    assert!(bad.is_empty(), "{} disagreements", bad.len());
}

async fn mysql_family(env: &str) {
    let Ok(url) = std::env::var(env) else {
        eprintln!("{env} unset: skipped");
        return;
    };
    let mut conn = sqlx::MySqlConnection::connect(&url).await.unwrap();
    let version: String = sqlx::query_scalar("SELECT VERSION()").fetch_one(&mut conn).await.unwrap();
    println!("== {env}: {version}");
    sqlx::raw_sql(
        "SET SESSION sql_mode = 'ALLOW_INVALID_DATES';
         CREATE TEMPORARY TABLE t (id INT, dt6 DATETIME(6), ts6 TIMESTAMP(6) NULL, d DATE, tm6 TIME(6), dt0 DATETIME, tm0 TIME);
         INSERT INTO t VALUES
          (1, '2026-10-15 09:30:00.5', '2026-10-15 09:30:00.5', '2026-10-15', '09:30:00.5', '2026-10-15 09:30:00', '09:30:00'),
          (2, '2026-10-15 00:00:00', '2026-10-15 00:00:00', '2026-10-15', '00:00:00', '2026-10-15 00:00:00', '00:00:00'),
          (3, '1000-01-01 00:00:00.000001', '1970-01-01 00:00:01', '1000-01-01', '-12:00:00', '1000-01-01 00:00:00', '-00:00:01'),
          (4, '9999-12-31 23:59:59.999999', '2038-01-19 03:14:07.999999', '9999-12-31', '838:59:59', '9999-12-31 23:59:59', '24:00:00'),
          (5, '0000-00-00 00:00:00', '0000-00-00 00:00:00', '0000-00-00', '23:59:59.999999', '0000-00-00 00:00:00', '-838:59:59'),
          (6, '2026-02-30 12:00:00', NULL, '2026-02-30', '00:00:00.000001', '2026-02-31 00:00:00', '12:34:56'),
          (7, '0999-12-31 23:59:59', NULL, '0099-01-01', '00:00:00', '0001-01-01 00:00:00', '00:00:00'),
          (8, '2040-02-29 23:59:59.5', NULL, '2040-02-29', '00:00:00.5', '2040-02-29 23:59:59', '01:00:00');",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let sql = "SELECT id, dt6, ts6, d, tm6, dt0, tm0 FROM t ORDER BY id";
    let binary = sqlx::query(sql).fetch_all(&mut conn).await.unwrap();
    let text = sqlx::raw_sql(sql).fetch_all(&mut conn).await.unwrap();
    let mut bad = Vec::new();
    for (protocol, rows) in [("binary", &binary), ("text", &text)] {
        for row in rows.iter() {
            let id: i32 = row.get("id");
            let label = format!("{env} {protocol} row {id}");
            let checks = vec![
                ("dt6 Timestamp", compare(row, "dt6", |v: Timestamp| *v, |c: CDateTime<Utc>| j_ts(c))),
                ("dt6 DateTime", compare(row, "dt6", |v: DateTime| *v, |c: NaiveDateTime| j_dt(c))),
                ("ts6 Timestamp", compare(row, "ts6", |v: Option<Timestamp>| v.map(|v| *v), |c: Option<CDateTime<Utc>>| Some(c.and_then(j_ts)))),
                ("d Date", compare(row, "d", |v: Date| *v, |c: NaiveDate| j_date(c))),
                ("tm6 Time", compare(row, "tm6", |v: Time| *v, |c: NaiveTime| Some(j_time(c)))),
                ("dt0 DateTime", compare(row, "dt0", |v: DateTime| *v, |c: NaiveDateTime| j_dt(c))),
                ("tm0 Time", compare(row, "tm0", |v: Time| *v, |c: NaiveTime| Some(j_time(c)))),
            ];
            report(&label, row, checks, &mut bad);
        }
    }

    // Writes: the PR's types and chrono's store the same value. The instant
    // goes as the MariaDB wire (`DateTime` of the UTC wall time) and also as
    // a raw `Timestamp` (SQLx's TIMESTAMP parameter type) to show why.
    sqlx::raw_sql("CREATE TEMPORARY TABLE w (k INT, a DATETIME(6) NULL, b DATETIME(6) NULL, c DATETIME(6) NULL, e DATETIME(6) NULL, da DATE NULL, db DATE NULL, ta TIME(6) NULL, tb TIME(6) NULL)")
        .execute(&mut conn)
        .await
        .unwrap();
    let instants = [
        "2026-10-15T09:30:00.5Z",
        "2040-02-29T23:59:59.999999Z",
        "1969-07-20T20:17:40Z",
        "1000-01-01T00:00:00.000000500Z",
        "9999-12-30T22:00:00.999999999Z",
        "2026-10-15T00:00:00Z",
        "2026-10-15T09:30:00.000000999Z",
    ];
    for (k, text) in instants.iter().enumerate() {
        let ours: Timestamp = text.parse().unwrap();
        let chrono = CDateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc);
        let wall = DateTime::from(jiff::tz::Offset::UTC.to_datetime(*ours));
        sqlx::query("INSERT INTO w VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)")
            .bind(k as i32)
            .bind(chrono.naive_utc())
            .bind(wall)
            .bind(chrono)
            .bind(ours)
            .bind(chrono.naive_utc().date())
            .bind(Date::from(wall.date()))
            .bind(chrono.naive_utc().time())
            .bind(Time::from(wall.time()))
            .execute(&mut conn)
            .await
            .unwrap();
    }
    let rows = sqlx::query(
        "SELECT k, CAST(a AS CHAR) a, CAST(b AS CHAR) b, CAST(c AS CHAR) c, CAST(e AS CHAR) e, \
         CAST(da AS CHAR) da, CAST(db AS CHAR) db, CAST(ta AS CHAR) ta, CAST(tb AS CHAR) tb FROM w ORDER BY k",
    )
    .fetch_all(&mut conn)
    .await
    .unwrap();
    for row in &rows {
        let s = |c: &str| row.get::<Option<String>, _>(c).unwrap_or_else(|| "NULL".into());
        let line = format!(
            "{env} write {}: wire chrono={} ours={} | TIMESTAMP-typed chrono={} ours={} | date {} {} | time {} {}",
            instants[row.get::<i32, _>("k") as usize],
            s("a"), s("b"), s("c"), s("e"), s("da"), s("db"), s("ta"), s("tb")
        );
        let agree = s("a") == s("b") && s("c") == s("e") && s("da") == s("db") && s("ta") == s("tb");
        println!("{} {line}", if agree { "  " } else { "!!" });
        if !agree {
            bad.push(line);
        }
    }
    assert!(bad.is_empty(), "{env}: {} disagreements", bad.len());
}

#[tokio::test]
async fn review_mysql_binary_and_text_reads_match_chrono() {
    mysql_family("REVIEW_MYSQL_URL").await;
}

#[tokio::test]
async fn review_mariadb_binary_and_text_reads_match_chrono() {
    mysql_family("REVIEW_MARIADB_URL").await;
}
