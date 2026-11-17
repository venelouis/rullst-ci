// REVIEW-ONLY differential tests (PR #754 review, not for merge).

use chrono::{DateTime as CDateTime, NaiveDate, NaiveDateTime, NaiveTime, Timelike, Utc};
use jiff::civil;
use sqlx::mysql::types::{MySqlTime, MySqlTimeSign};
use sqlx::{Encode, MySql, Type};

use super::mysql::{date_time_from_binary, time_from_mysql_time};
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

fn bytes<T: for<'q> Encode<'q, MySql>>(value: &T) -> Result<Vec<u8>, String> {
    let mut buf = Vec::new();
    Encode::<MySql>::encode_by_ref(value, &mut buf)
        .map(|_| buf)
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

#[test]
fn review_mysql_encoders_match_sqlx_chrono() {
    let mut rng = Rng(0xA076_1D64_78BD_642F);
    let min = jiff::Timestamp::MIN.as_second();
    let max = jiff::Timestamp::MAX.as_second();
    let mut mismatches = Vec::new();
    let mut cases = 0;
    for i in 0..300_000 {
        let secs = match i % 3 {
            0 => rng.range(min, max),
            1 => rng.range(-62_167_219_200 - 100_000, -30_000_000_000), // years -1..1000
            _ => rng.range(-2_000_000_000, 2_200_000_000),
        };
        let nanos: u32 = match rng.below(5) {
            0 => 0,
            1 => (rng.below(1_000_000) * 1000) as u32,
            2 => rng.below(1_000_000_000) as u32,
            3 => rng.below(1000) as u32, // sub-microsecond only
            _ => {
                // midnight exactly
                let day = secs.rem_euclid(86_400);
                let _ = day;
                0
            }
        };
        let secs = if rng.below(10) == 0 { secs - secs.rem_euclid(86_400) } else { secs };
        let total = i128::from(secs) * 1_000_000_000 + i128::from(nanos);
        let Ok(ts) = jiff::Timestamp::from_nanosecond(total) else {
            continue;
        };
        cases += 1;
        let chrono = CDateTime::<Utc>::from_timestamp(secs, nanos).unwrap();
        let wall = crate::temporal::text::utc(ts);
        let naive = chrono.naive_utc();
        let checks = [
            ("timestamp", bytes(&Timestamp::from(ts)), bytes(&chrono)),
            // The MariaDB wire for an instant: `DateTime::from(text::utc(ts))`
            // against chrono's `naive_utc()` (`__bind_field::wire`).
            ("datetime", bytes(&DateTime::from(wall)), bytes(&naive)),
            ("date", bytes(&Date::from(wall.date())), bytes(&naive.date())),
            ("time", bytes(&Time::from(wall.time())), bytes(&naive.time())),
        ];
        for (what, ours, theirs) in checks {
            // Error messages differ in wording; compare Ok bytes and Err-ness.
            let same = match (&ours, &theirs) {
                (Ok(a), Ok(b)) => a == b,
                (Err(_), Err(_)) => true,
                _ => false,
            };
            if !same {
                mismatches.push(format!("{what} {ts}: ours={ours:?} chrono={theirs:?}"));
            }
        }
    }
    println!("[mysql encode] cases={cases} mismatches={}", mismatches.len());
    for m in mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}

// sqlx-mysql 0.9.0 binary `NaiveDateTime` decode, ported line by line
// (`Err(())` stands for both an error and a panic there).
fn sqlx_decode_date(buf: &[u8]) -> Result<Option<NaiveDate>, ()> {
    match buf.len() {
        0 => Ok(None),
        4.. => {
            let year = i32::from(u16::from_le_bytes([buf[0], buf[1]]));
            NaiveDate::from_ymd_opt(year, u32::from(buf[2]), u32::from(buf[3]))
                .map(Some)
                .ok_or(())
        }
        _ => Err(()),
    }
}

fn sqlx_decode_time(len: u8, buf: &[u8]) -> Result<NaiveTime, ()> {
    if buf.len() < 3 {
        return Err(()); // `get_u8` panics
    }
    let rest = &buf[3..];
    let micros = if len > 3 {
        if rest.len() > 8 {
            return Err(()); // `get_uint_le` panics
        }
        let mut le = [0; 8];
        le[..rest.len()].copy_from_slice(rest);
        u64::from_le_bytes(le)
    } else {
        0
    };
    let micros = u32::try_from(micros).map_err(|_| ())?;
    NaiveTime::from_hms_micro_opt(u32::from(buf[0]), u32::from(buf[1]), u32::from(buf[2]), micros)
        .ok_or(())
}

fn sqlx_decode_date_time(buf: &[u8]) -> Result<NaiveDateTime, ()> {
    if buf.is_empty() {
        return Err(());
    }
    let len = buf[0];
    let date = sqlx_decode_date(&buf[1..])?.ok_or(())?;
    if len > 4 {
        Ok(date.and_time(sqlx_decode_time(len - 4, &buf[5..])?))
    } else {
        Ok(date.and_hms_opt(0, 0, 0).unwrap())
    }
}

#[test]
fn review_mysql_binary_date_time_decoder_matches_sqlx_chrono() {
    let mut rng = Rng(0xE703_7ED1_A0B4_28DB);
    let mut mismatches = Vec::new();
    for _ in 0..300_000 {
        let len = [0u8, 4, 7, 11, 4, 7, 11, 5, 8, 12, 3][rng.below(11)];
        let year = [0u16, 1, 999, 1000, 2026, 9999, 10_000, 32_767, 32_768, 65_535]
            [rng.below(10)];
        let mut buf = vec![len];
        buf.extend_from_slice(&year.to_le_bytes());
        buf.push([0u8, 1, 2, 12, 13, 255][rng.below(6)]);
        buf.push([0u8, 1, 28, 29, 30, 31, 32][rng.below(7)]);
        buf.push([0u8, 9, 23, 24][rng.below(4)]);
        buf.push([0u8, 30, 59, 60][rng.below(4)]);
        buf.push([0u8, 30, 59, 60][rng.below(4)]);
        let micros: u64 = [0, 1, 500_000, 999_999, 1_000_000, 1_999_999, 2_000_000, u64::MAX]
            [rng.below(8)];
        buf.extend_from_slice(&micros.to_le_bytes()[..[0, 1, 3, 4, 8][rng.below(5)]]);
        if rng.below(4) == 0 {
            buf.push(0);
        }
        let cut = if rng.below(5) == 0 { rng.below(buf.len() + 1) } else { buf.len() };
        let buf = &buf[..cut];
        let ours = date_time_from_binary(buf).ok();
        let theirs = sqlx_decode_date_time(buf).ok();
        let ok = match (&ours, &theirs) {
            (None, None) => true,
            (None, Some(n)) => civil_dt(n).is_none(),
            (Some(a), Some(n)) => civil_dt(n).as_ref() == Some(a),
            (Some(_), None) => false,
        };
        if !ok {
            mismatches.push(format!("{buf:?}: ours={ours:?} sqlx={theirs:?}"));
        }
    }
    println!("[mysql binary datetime] mismatches={}", mismatches.len());
    for m in mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}

#[test]
fn review_mysql_time_matches_sqlx_try_from() {
    let mut rng = Rng(0x8EBC_6AF0_9C88_C6E3);
    let mut mismatches = Vec::new();
    for _ in 0..300_000 {
        let sign = if rng.below(2) == 0 { MySqlTimeSign::Positive } else { MySqlTimeSign::Negative };
        let hours = if rng.below(2) == 0 { rng.below(24) } else { rng.below(839) } as u32;
        let Ok(time) = MySqlTime::new(
            sign,
            hours,
            rng.below(60) as u8,
            rng.below(60) as u8,
            [0u32, 1, 999_999, rng.below(1_000_000) as u32][rng.below(4)],
        ) else {
            continue;
        };
        let ours = time_from_mysql_time(&time);
        let theirs = NaiveTime::try_from(time).ok().map(|t| {
            civil::Time::new(t.hour() as i8, t.minute() as i8, t.second() as i8, t.nanosecond() as i32)
                .unwrap()
        });
        if ours != theirs {
            mismatches.push(format!("{time}: ours={ours:?} sqlx={theirs:?}"));
        }
    }
    println!("[mysql time] mismatches={}", mismatches.len());
    for m in mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}

#[test]
fn review_mysql_type_infos_match_chrono() {
    use sqlx::TypeInfo;
    let infos = [
        <CDateTime<Utc> as Type<MySql>>::type_info(),
        <NaiveDateTime as Type<MySql>>::type_info(),
        <NaiveDate as Type<MySql>>::type_info(),
        <NaiveTime as Type<MySql>>::type_info(),
        <String as Type<MySql>>::type_info(),
        <i64 as Type<MySql>>::type_info(),
        <Vec<u8> as Type<MySql>>::type_info(),
    ];
    assert_eq!(<Timestamp as Type<MySql>>::type_info(), infos[0]);
    assert_eq!(<DateTime as Type<MySql>>::type_info(), infos[1]);
    assert_eq!(<Date as Type<MySql>>::type_info(), infos[2]);
    assert_eq!(<Time as Type<MySql>>::type_info(), infos[3]);
    for info in &infos {
        let name = info.name();
        assert_eq!(
            <Timestamp as Type<MySql>>::compatible(info),
            <CDateTime<Utc> as Type<MySql>>::compatible(info),
            "{name}"
        );
        assert_eq!(
            <DateTime as Type<MySql>>::compatible(info),
            <NaiveDateTime as Type<MySql>>::compatible(info),
            "{name}"
        );
        assert_eq!(
            <Date as Type<MySql>>::compatible(info),
            <NaiveDate as Type<MySql>>::compatible(info),
            "{name}"
        );
        assert_eq!(
            <Time as Type<MySql>>::compatible(info),
            <NaiveTime as Type<MySql>>::compatible(info),
            "{name}"
        );
    }
}
