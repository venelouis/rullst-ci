// REVIEW-ONLY differential tests (PR #754 review, not for merge): a real
// in-memory SQLite through sqlx, chrono's codecs against the PR's.

use chrono::{DateTime as CDateTime, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};
use jiff::civil;
use sqlx::sqlite::SqliteRow;
use sqlx::{Connection, Row, SqliteConnection, Type, TypeInfo};

use super::{Date, DateTime, Time, Timestamp};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = (hi as i128 - lo as i128 + 1) as u128;
        (lo as i128 + (self.next() as u128 % span) as i128) as i64
    }
}

fn civil_time(t: &NaiveTime) -> civil::Time {
    civil::Time::new(
        t.hour() as i8,
        t.minute() as i8,
        t.second() as i8,
        (t.nanosecond() % 1_000_000_000) as i32,
    )
    .unwrap()
}

fn civil_dt(n: &NaiveDateTime) -> Option<civil::DateTime> {
    let date = civil::Date::new(
        i16::try_from(chrono::Datelike::year(n)).ok()?,
        chrono::Datelike::month(n) as i8,
        chrono::Datelike::day(n) as i8,
    )
    .ok()?;
    Some(date.to_datetime(civil_time(&n.time())))
}

/// Compares one decode: chrono `Ok` outside jiff's range may be our `Err`.
fn same<T: PartialEq + std::fmt::Debug>(
    ours: Result<T, sqlx::Error>,
    theirs: Result<Option<T>, sqlx::Error>,
) -> Result<(), String> {
    match (ours, theirs) {
        (Err(_), Err(_)) | (Err(_), Ok(None)) => Ok(()),
        (Ok(a), Ok(Some(b))) if a == b => Ok(()),
        (a, b) => Err(format!("ours={a:?} chrono={b:?}")),
    }
}

fn compare_row(row: &SqliteRow, column: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |what: &str, r: Result<(), String>| {
        if let Err(e) = r {
            out.push(format!("{column} as {what}: {e}"));
        }
    };
    push(
        "Timestamp",
        same(
            row.try_get::<Timestamp, _>(column).map(jiff::Timestamp::from),
            row.try_get::<CDateTime<Utc>, _>(column).map(|d| {
                civil_dt(&d.naive_utc()).and_then(|c| jiff::tz::Offset::UTC.to_timestamp(c).ok())
            }),
        ),
    );
    push(
        "DateTime",
        same(
            row.try_get::<DateTime, _>(column).map(civil::DateTime::from),
            row.try_get::<NaiveDateTime, _>(column).map(|d| civil_dt(&d)),
        ),
    );
    push(
        "Date",
        same(
            row.try_get::<Date, _>(column).map(civil::Date::from),
            row.try_get::<NaiveDate, _>(column).map(|d| civil_dt(&d.and_hms_opt(0, 0, 0).unwrap()).map(|c| c.date())),
        ),
    );
    push(
        "Time",
        same(
            row.try_get::<Time, _>(column).map(civil::Time::from),
            row.try_get::<NaiveTime, _>(column).map(|t| Some(civil_time(&t))),
        ),
    );
    out
}

const TEXTS: &[&str] = &[
    "2026-10-15 09:30:00",
    "2026-10-15 09:30:00.5",
    "2026-10-15T09:30:00.500+00:00",
    "2026-10-15T09:30:00.5-03:00",
    "2026-10-15T09:30:00Z",
    "2026-10-15 09:30",
    "2026-10-15T09:30Z",
    "2026-10-15 09:30+05:30",
    "2026-10-15T09:30:00.123456789123+00:00",
    "2016-12-31T23:59:60Z",
    "2016-12-31 23:59:60.5",
    "2026-10-15T09:30:00+24:00",
    "2026-10-15 09:30:00+99:59",
    "9999-12-31T23:59:59+00:00",
    "9999-12-31 23:59:59",
    "-0001-01-01 00:00:00",
    "+12026-01-01 00:00:00",
    "2026-10-15",
    "09:30:00",
    "09:30",
    "09:30:00.5Z",
    "09:30:00.f",
    "09:30+01:00",
    "garbage",
    "",
    " 2026-10-15 09:30:00",
    "2026-10-15  09:30:00",
    "2026-10-15T09:30:00.5\u{2212}03:00",
];

#[tokio::test]
async fn review_sqlite_live_matches_chrono() {
    let mut conn = SqliteConnection::connect("sqlite::memory:").await.unwrap();
    let mut mismatches = Vec::new();
    let mut rng = Rng(0xBF58_476D_1CE4_E5B9);
    let min = jiff::Timestamp::MIN.as_second();
    let max = jiff::Timestamp::MAX.as_second();

    // 1. Writes: the bound text and its storage class.
    for i in 0..5_000 {
        let secs = if i % 2 == 0 { rng.range(min, max) } else { rng.range(-3_000_000_000, 3_000_000_000) };
        let nanos = [0u32, 500_000_000, 123_456_000, 999_999_999, rng.below(1_000_000_000) as u32][rng.below(5)];
        let Ok(ts) = jiff::Timestamp::from_nanosecond(i128::from(secs) * 1_000_000_000 + i128::from(nanos)) else {
            continue;
        };
        let chrono = CDateTime::<Utc>::from_timestamp(secs, nanos).unwrap();
        let wall = crate::temporal::text::utc(ts);
        let row = sqlx::query(
            "SELECT ?1 AS a, typeof(?1) AS ta, ?2 AS b, typeof(?2) AS tb, \
                    ?3 AS c, ?4 AS d, ?5 AS e, ?6 AS f, ?7 AS g, ?8 AS h",
        )
        .bind(chrono)
        .bind(Timestamp::from(ts))
        .bind(chrono.naive_utc())
        .bind(DateTime::from(wall))
        .bind(chrono.naive_utc().date())
        .bind(Date::from(wall.date()))
        .bind(chrono.naive_utc().time())
        .bind(Time::from(wall.time()))
        .fetch_one(&mut conn)
        .await
        .unwrap();
        for (x, y) in [("a", "b"), ("ta", "tb"), ("c", "d"), ("e", "f"), ("g", "h")] {
            let (x, y): (String, String) = (row.get(x), row.get(y));
            if x != y {
                mismatches.push(format!("write {ts}: chrono={x:?} ours={y:?}"));
            }
        }
    }

    // 2. Reads from TEXT, INTEGER and REAL storage, and NULL.
    let mut integers = vec![0_i64, 1, -1, i64::MAX, i64::MIN, 253_402_207_200, 253_402_300_799, -377_705_116_800];
    for _ in 0..2_000 {
        integers.push(rng.range(-400_000_000_000, 400_000_000_000));
    }
    let mut reals = vec![
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.0,
        -0.0,
        2_440_587.5,
        2_440_587.499_999_9,
        2_451_545.0,
        -1.5,
        1e300,
        5_373_484.0,
        5_373_484.5,
        -1_000_000.25,
    ];
    for _ in 0..2_000 {
        reals.push((rng.next() % 6_000_000_000) as f64 / 1000.0 - 1_000_000.0);
    }
    for text in TEXTS {
        let row = sqlx::query("SELECT ? AS v").bind(*text).fetch_one(&mut conn).await.unwrap();
        mismatches.extend(compare_row(&row, "v").into_iter().map(|m| format!("text {text:?} {m}")));
    }
    for integer in &integers {
        let row = sqlx::query("SELECT ? AS v").bind(*integer).fetch_one(&mut conn).await.unwrap();
        mismatches.extend(compare_row(&row, "v").into_iter().map(|m| format!("integer {integer} {m}")));
    }
    for real in &reals {
        let row = sqlx::query("SELECT ? AS v").bind(*real).fetch_one(&mut conn).await.unwrap();
        mismatches.extend(compare_row(&row, "v").into_iter().map(|m| format!("real {real} {m}")));
    }

    // 3. Declared column types, NULL and non-NULL, through `compatible()`.
    sqlx::raw_sql(
        "CREATE TABLE t (c_dt DATETIME, c_ts TIMESTAMP, c_text TEXT, c_int INTEGER, c_real REAL, \
         c_date DATE, c_time TIME, c_bool BOOLEAN, c_num NUMERIC, c_blob BLOB, c_none);
         INSERT INTO t VALUES ('2026-10-15 09:30:00', '2026-10-15 09:30:00', '09:30:00', 1792056600, \
         2461328.89583, '2026-10-15', '09:30:00', 1, 20261015, x'00', '2026-10-15');
         INSERT INTO t VALUES (NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL, NULL);",
    )
    .execute(&mut conn)
    .await
    .unwrap();
    let rows = sqlx::query("SELECT * FROM t").fetch_all(&mut conn).await.unwrap();
    for (index, row) in rows.iter().enumerate() {
        for column in ["c_dt", "c_ts", "c_text", "c_int", "c_real", "c_date", "c_time", "c_bool", "c_num", "c_blob", "c_none"] {
            mismatches.extend(compare_row(row, column).into_iter().map(|m| format!("row {index} {m}")));
            let ours = row.try_get::<Option<Timestamp>, _>(column).is_ok();
            let theirs = row.try_get::<Option<CDateTime<Utc>>, _>(column).is_ok();
            if ours != theirs {
                mismatches.push(format!("row {index} {column} Option<Timestamp>: ours ok={ours} chrono ok={theirs}"));
            }
        }
    }

    // 4. `compatible()` for every type SQLx names.
    let infos = [
        <String as Type<sqlx::Sqlite>>::type_info(),
        <i64 as Type<sqlx::Sqlite>>::type_info(),
        <i32 as Type<sqlx::Sqlite>>::type_info(),
        <f64 as Type<sqlx::Sqlite>>::type_info(),
        <bool as Type<sqlx::Sqlite>>::type_info(),
        <Vec<u8> as Type<sqlx::Sqlite>>::type_info(),
        <NaiveDate as Type<sqlx::Sqlite>>::type_info(),
        <NaiveTime as Type<sqlx::Sqlite>>::type_info(),
        <NaiveDateTime as Type<sqlx::Sqlite>>::type_info(),
    ];
    for info in &infos {
        let checks = [
            ("Timestamp", <Timestamp as Type<sqlx::Sqlite>>::compatible(info), <CDateTime<Utc> as Type<sqlx::Sqlite>>::compatible(info)),
            ("DateTime", <DateTime as Type<sqlx::Sqlite>>::compatible(info), <NaiveDateTime as Type<sqlx::Sqlite>>::compatible(info)),
            ("Date", <Date as Type<sqlx::Sqlite>>::compatible(info), <NaiveDate as Type<sqlx::Sqlite>>::compatible(info)),
            ("Time", <Time as Type<sqlx::Sqlite>>::compatible(info), <NaiveTime as Type<sqlx::Sqlite>>::compatible(info)),
        ];
        for (what, ours, theirs) in checks {
            if ours != theirs {
                mismatches.push(format!("compatible {what} {}: ours={ours} chrono={theirs}", info.name()));
            }
        }
    }
    assert_eq!(<Timestamp as Type<sqlx::Sqlite>>::type_info(), <CDateTime<Utc> as Type<sqlx::Sqlite>>::type_info());
    assert_eq!(<DateTime as Type<sqlx::Sqlite>>::type_info(), <NaiveDateTime as Type<sqlx::Sqlite>>::type_info());
    assert_eq!(<Date as Type<sqlx::Sqlite>>::type_info(), <NaiveDate as Type<sqlx::Sqlite>>::type_info());
    assert_eq!(<Time as Type<sqlx::Sqlite>>::type_info(), <NaiveTime as Type<sqlx::Sqlite>>::type_info());

    println!("[sqlite live] mismatches={}", mismatches.len());
    for m in mismatches.iter().take(40) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}
