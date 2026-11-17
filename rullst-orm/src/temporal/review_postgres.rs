// REVIEW-ONLY differential tests (PR #754 review, not for merge).

use chrono::{
    DateTime as CDateTime, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeDelta, TimeZone,
    Timelike, Utc,
};
use jiff::civil;
use sqlx::postgres::PgArgumentBuffer;
use sqlx::{Encode, Postgres};

use super::postgres::{date_from_days, date_time_from_micros, instant_from_micros, time_from_micros};
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

fn bytes<T: for<'q> Encode<'q, Postgres>>(value: &T) -> Result<Vec<u8>, String> {
    let mut buf = PgArgumentBuffer::default();
    Encode::<Postgres>::encode_by_ref(value, &mut buf)
        .map(|_| buf.to_vec())
        .map_err(|e| e.to_string())
}

fn civil_dt(n: &NaiveDateTime) -> Option<civil::DateTime> {
    let date = civil::Date::new(
        i16::try_from(chrono::Datelike::year(n)).ok()?,
        chrono::Datelike::month(n) as i8,
        chrono::Datelike::day(n) as i8,
    )
    .ok()?;
    Some(date.to_datetime(
        civil::Time::new(
            n.hour() as i8,
            n.minute() as i8,
            n.second() as i8,
            (n.nanosecond() % 1_000_000_000) as i32,
        )
        .unwrap(),
    ))
}

fn epoch() -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2000, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap()
}

#[test]
fn review_postgres_encoders_match_sqlx_chrono() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    let min = jiff::Timestamp::MIN.as_second();
    let max = jiff::Timestamp::MAX.as_second();
    let mut mismatches = Vec::new();
    let mut cases = 0;
    let mut special = vec![
        // pre-2000 sub-microsecond: truncation toward 2000
        (946_684_799_i64, 999_999_500_u32),
        (946_684_799, 999_999_999),
        (946_684_799, 1),
        (946_684_800, 500),
        (946_684_800, 999),
        (-1, 999_999_500),
        (0, 0),
        (min, 0),
        (max, 999_999_999),
    ];
    for i in 0..300_000 {
        let (secs, nanos) = if let Some(s) = special.pop() {
            s
        } else {
            let secs = match i % 3 {
                0 => rng.range(min, max),
                1 => rng.range(946_684_800 - 5_000_000, 946_684_800 + 5_000_000),
                _ => rng.range(-5_000_000, 5_000_000),
            };
            let nanos = match rng.below(4) {
                0 => 0,
                1 => (rng.below(1_000_000) * 1000) as u32,
                2 => rng.below(1_000_000_000) as u32,
                _ => (999_999_000 + rng.below(1000)) as u32,
            };
            (secs, nanos)
        };
        let total = i128::from(secs) * 1_000_000_000 + i128::from(nanos);
        let Ok(ts) = jiff::Timestamp::from_nanosecond(total) else {
            continue;
        };
        cases += 1;
        let chrono = CDateTime::<Utc>::from_timestamp(secs, nanos).unwrap();
        let wall = crate::temporal::text::utc(ts);
        let naive = chrono.naive_utc();
        let checks = [
            ("timestamptz", bytes(&Timestamp::from(ts)), bytes(&chrono)),
            ("timestamp", bytes(&DateTime::from(wall)), bytes(&naive)),
            ("date", bytes(&Date::from(wall.date())), bytes(&naive.date())),
            ("time", bytes(&Time::from(wall.time())), bytes(&naive.time())),
        ];
        for (what, ours, theirs) in checks {
            if ours != theirs {
                mismatches.push(format!("{what} {ts}: ours={ours:?} chrono={theirs:?}"));
            }
        }
    }
    println!("[pg encode] cases={cases} mismatches={}", mismatches.len());
    for m in mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}

/// Binary decoding. sqlx panics where chrono's `+` overflows
/// (`checked_add_signed` is `None` below); the PR must return `None` there and
/// for a chrono value outside jiff's range, and the same value otherwise.
#[test]
fn review_postgres_binary_decoders_match_sqlx_chrono() {
    let mut rng = Rng(0x94D0_49BB_1331_11EB);
    let mut mismatches = Vec::new();
    let mut micros = vec![
        0_i64,
        -1,
        1,
        i64::MAX,
        i64::MIN,
        86_400_000_000,
        -86_400_000_000,
        86_400_000_000 * 2 + 5,
    ];
    for i in 0..300_000 {
        let us = micros.pop().unwrap_or_else(|| match i % 3 {
            0 => rng.next() as i64,
            1 => rng.range(-400_000_000_000_000_000, 400_000_000_000_000_000),
            _ => rng.range(-10_000_000_000_000, 10_000_000_000_000),
        });
        let reference = epoch().checked_add_signed(Duration::microseconds(us));
        let expected = reference.map(|n| civil_dt(&n));
        let ours = date_time_from_micros(us);
        let ok = match (&ours, &expected) {
            (None, None | Some(None)) => true,
            (Some(a), Some(Some(b))) => a == b,
            _ => false,
        };
        if !ok {
            mismatches.push(format!("timestamp {us}: ours={ours:?} chrono={expected:?}"));
        }
        let ours_instant = instant_from_micros(us);
        let expected_instant = reference.map(|n| {
            civil_dt(&n).and_then(|c| jiff::tz::Offset::UTC.to_timestamp(c).ok())
        });
        let ok = match (&ours_instant, &expected_instant) {
            (None, None | Some(None)) => true,
            (Some(a), Some(Some(b))) => a == b,
            _ => false,
        };
        if !ok {
            mismatches.push(format!(
                "timestamptz {us}: ours={ours_instant:?} chrono={expected_instant:?}"
            ));
        }
        // TIME wraps in both.
        let t = NaiveTime::default() + Duration::microseconds(us);
        let theirs = civil::Time::new(
            t.hour() as i8,
            t.minute() as i8,
            t.second() as i8,
            t.nanosecond() as i32,
        )
        .unwrap();
        if time_from_micros(us) != theirs {
            mismatches.push(format!(
                "time {us}: ours={:?} chrono={theirs:?}",
                time_from_micros(us)
            ));
        }
        let days = (rng.next() as u32) as i32;
        let days = if i % 2 == 0 { days } else { days % 4_000_000 };
        let reference = NaiveDate::from_ymd_opt(2000, 1, 1)
            .unwrap()
            .checked_add_signed(TimeDelta::try_days(days.into()).unwrap());
        let expected = reference.map(|d| {
            civil::Date::new(
                i16::try_from(chrono::Datelike::year(&d)).ok()?,
                chrono::Datelike::month(&d) as i8,
                chrono::Datelike::day(&d) as i8,
            )
            .ok()
        });
        let ours = date_from_days(days);
        let ok = match (&ours, &expected) {
            (None, None | Some(None)) => true,
            (Some(a), Some(Some(b))) => a == b,
            _ => false,
        };
        if !ok {
            mismatches.push(format!("date {days}: ours={ours:?} chrono={expected:?}"));
        }
    }
    println!("[pg decode] mismatches={}", mismatches.len());
    for m in mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}

#[test]
fn review_postgres_type_infos_match_chrono() {
    use sqlx::Type;
    use sqlx::postgres::PgHasArrayType;
    assert_eq!(
        <Timestamp as Type<Postgres>>::type_info(),
        <CDateTime<Utc> as Type<Postgres>>::type_info()
    );
    assert_eq!(
        <DateTime as Type<Postgres>>::type_info(),
        <NaiveDateTime as Type<Postgres>>::type_info()
    );
    assert_eq!(<Date as Type<Postgres>>::type_info(), <NaiveDate as Type<Postgres>>::type_info());
    assert_eq!(<Time as Type<Postgres>>::type_info(), <NaiveTime as Type<Postgres>>::type_info());
    assert_eq!(
        <Timestamp as PgHasArrayType>::array_type_info(),
        <CDateTime<Utc> as PgHasArrayType>::array_type_info()
    );
    assert_eq!(
        <DateTime as PgHasArrayType>::array_type_info(),
        <NaiveDateTime as PgHasArrayType>::array_type_info()
    );
    assert_eq!(
        <Date as PgHasArrayType>::array_type_info(),
        <NaiveDate as PgHasArrayType>::array_type_info()
    );
    assert_eq!(
        <Time as PgHasArrayType>::array_type_info(),
        <NaiveTime as PgHasArrayType>::array_type_info()
    );
    let _ = Utc.timestamp_opt(0, 0);
}
