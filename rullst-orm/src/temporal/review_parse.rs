// REVIEW-ONLY differential tests (PR #754 review, not for merge). Each test
// compares the PR's code with chrono 0.4.45 driven exactly as sqlx 0.9.0's
// chrono codecs drive it (the glue is copied verbatim from sqlx's sources).

use chrono::{
    DateTime as CDateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, NaiveTime, Offset,
    SecondsFormat, TimeZone, Timelike, Utc,
};
use jiff::civil;

use super::parse::{DATE_ONLY, DATE_TIME, TIME, parse, sqlite_date_time, sqlite_time};
use super::{Date, DateTime, Time, Timestamp, text};

pub(super) struct Rng(pub(super) u64);

impl Rng {
    pub(super) fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub(super) fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    pub(super) fn pick<T: Copy>(&mut self, items: &[T]) -> T {
        items[self.below(items.len())]
    }
    pub(super) fn range(&mut self, lo: i64, hi: i64) -> i64 {
        let span = (hi as i128 - lo as i128 + 1) as u128;
        (lo as i128 + (self.next() as u128 % span) as i128) as i64
    }
}

// ---- sqlx 0.9.0 glue, verbatim ------------------------------------------

/// sqlx-sqlite `decode_datetime_from_text`.
fn sqlx_sqlite_datetime(value: &str) -> Option<CDateTime<FixedOffset>> {
    if let Ok(dt) = CDateTime::parse_from_rfc3339(value) {
        return Some(dt);
    }
    let formats = [
        "%F %T%.f", "%F %R", "%F %RZ", "%F %R%:z", "%F %T%.fZ", "%F %T%.f%:z", "%FT%R", "%FT%RZ",
        "%FT%R%:z", "%FT%T%.f", "%FT%T%.fZ", "%FT%T%.f%:z",
    ];
    for format in formats {
        if let Ok(dt) = CDateTime::parse_from_str(value, format) {
            return Some(dt);
        }
        if let Ok(dt) = NaiveDateTime::parse_from_str(value, format) {
            return Some(Utc.fix().from_utc_datetime(&dt));
        }
    }
    None
}

/// sqlx-sqlite `Decode for NaiveTime`.
fn sqlx_sqlite_time(value: &str) -> Option<NaiveTime> {
    for format in ["%T.f", "%T%.f", "%R", "%RZ", "%T%.fZ", "%R%:z", "%T%.f%:z"] {
        if let Ok(t) = NaiveTime::parse_from_str(value, format) {
            return Some(t);
        }
    }
    None
}

/// sqlx-postgres text `DateTime<FixedOffset>`.
fn sqlx_pg_timestamptz(s: &str) -> Option<CDateTime<FixedOffset>> {
    CDateTime::parse_from_str(
        s,
        if s.contains('+') || s.contains('-') {
            "%Y-%m-%d %H:%M:%S%.f%#z"
        } else {
            "%Y-%m-%d %H:%M:%S%.f"
        },
    )
    .ok()
}

/// sqlx-postgres text `NaiveDateTime`.
fn sqlx_pg_timestamp(s: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(
        s,
        if s.contains('+') {
            "%Y-%m-%d %H:%M:%S%.f%#z"
        } else {
            "%Y-%m-%d %H:%M:%S%.f"
        },
    )
    .ok()
}

// ---- chrono -> jiff, with a leap second read as second 59 ------------------

pub(super) fn civil_time(t: &NaiveTime) -> civil::Time {
    civil::Time::new(
        t.hour() as i8,
        t.minute() as i8,
        t.second() as i8,
        (t.nanosecond() % 1_000_000_000) as i32,
    )
    .unwrap()
}

pub(super) fn civil_date(d: &NaiveDate) -> Option<civil::Date> {
    civil::Date::new(i16::try_from(d.year()).ok()?, d.month() as i8, d.day() as i8).ok()
}

pub(super) fn civil_dt(n: &NaiveDateTime) -> Option<civil::DateTime> {
    Some(civil_date(&n.date())?.to_datetime(civil_time(&n.time())))
}

fn instant_of(n: &NaiveDateTime) -> Option<jiff::Timestamp> {
    jiff::tz::Offset::UTC.to_timestamp(civil_dt(n)?).ok()
}

// ---- input generator -----------------------------------------------------

const YEARS: &[&str] = &[
    "2026", "1999", "2000", "1970", "0000", "0001", "9999", "-0001", "+2026", "-9999", "-10000",
    "+10000", "+002026", " 2026", "26", "202", "20261", "-1", "+1", "1", "  0987", "-0000", "\t2026",
];
const MONTHS: &[&str] = &["01", "1", "10", "12", "13", "00", " 1", "  10", "011", "+1", "-1", "02"];
const DAYS: &[&str] = &["01", "15", "31", "30", "29", "28", "32", "1", " 5", "00", "010"];
const SEPS: &[&str] = &[
    " ", "T", "t", "  ", "\t", "", "T ", " T", "\u{3000}", "\u{a0}", "x", "\n", " ", "T",
];
const HOURS: &[&str] = &["00", "09", "23", "24", "9", " 9", "123", "+9", "12"];
const MINUTES: &[&str] = &["00", "30", "59", "60", "5", " 5"];
const SECONDS: &[&str] = &["00", "59", "60", "61", "5", " 5", "09", "30"];
const FRACTIONS: &[&str] = &[
    "", "", ".", ".5", ".50", ".500", ".000001", ".000000500", ".123456789", ".1234567891234",
    ".0000000001", "..5", ".5a", ". 5", ".f", ".999999999", ".9999999999", ",5",
];
const OFFSETS: &[&str] = &[
    "", "", "Z", "z", "+00:00", "+00", "+0000", "-03:00", "-0300", "-03", "+05:30", "+5:30",
    "+23:59", "+24:00", "+99:59", "+99", "\u{2212}03:00", " +00:00", "+00 :00", "+00: 00",
    "+00::00", " Z", "UTC", " UTC", "+00:60", "+00:5", "+0", "-00:00", "+00:00:00", "Z ",
    "+00:00 ", "+01\t30", "-23:59", "+12", "\u{2212}03", "-24:00",
];
const TRAILING: &[&str] = &["", "", "", "", " ", "x", "\n", "0"];
const ALPHABET: &[char] = &[
    '0', '1', '5', '9', '-', ':', '.', '+', ' ', 'T', 'Z', 't', 'z', '\u{2212}', 'é', '/', '\t',
];

fn mutate(rng: &mut Rng, mut s: String) -> String {
    for _ in 0..1 + rng.below(2) {
        let indices: Vec<usize> = s.char_indices().map(|(i, _)| i).collect();
        if indices.is_empty() {
            break;
        }
        let at = indices[rng.below(indices.len())];
        let ch = s[at..].chars().next().unwrap();
        match rng.below(3) {
            0 => s.replace_range(at..at + ch.len_utf8(), &rng.pick(ALPHABET).to_string()),
            1 => s.insert(at, rng.pick(ALPHABET)),
            _ => {
                s.replace_range(at..at + ch.len_utf8(), "");
            }
        }
    }
    s
}

fn clock(rng: &mut Rng) -> String {
    let mut s = format!("{}:{}", rng.pick(HOURS), rng.pick(MINUTES));
    if rng.below(5) != 0 {
        s.push(':');
        s.push_str(rng.pick(SECONDS));
        s.push_str(rng.pick(FRACTIONS));
    }
    s
}

fn random_date_time_text(rng: &mut Rng) -> String {
    let mut s = String::new();
    if rng.below(3) == 0 {
        // A canonical rendering of a random valid value.
        let secs = rng.range(-62_000_000_000, 253_000_000_000);
        let nanos = rng.pick(&[0u32, 500_000_000, 123_456_000, 1, 999_999_999]);
        let at = CDateTime::from_timestamp(secs, nanos).unwrap();
        let fmt = rng.pick(&[
            "%F %T%.f",
            "%FT%T%.f",
            "%Y-%m-%d %H:%M:%S%.f",
            "%FT%T%.f%:z",
            "%F %R",
            "%FT%RZ",
            "%F %T%.fZ",
        ]);
        s = at.format(fmt).to_string();
        if fmt == "%Y-%m-%d %H:%M:%S%.f" {
            s.push_str(rng.pick(&["", "+00", "-05", "+05:30"]));
        }
        if rng.below(2) == 0 {
            s = at.to_rfc3339_opts(SecondsFormat::AutoSi, rng.below(2) == 0);
        }
    } else {
        s.push_str(rng.pick(YEARS));
        s.push_str(rng.pick(&["-", "-", "-", "-", "/", " -", ""]));
        s.push_str(rng.pick(MONTHS));
        s.push('-');
        s.push_str(rng.pick(DAYS));
        if rng.below(8) != 0 {
            s.push_str(rng.pick(SEPS));
            s.push_str(&clock(rng));
        }
        s.push_str(rng.pick(OFFSETS));
        s.push_str(rng.pick(TRAILING));
    }
    if rng.below(4) == 0 { mutate(rng, s) } else { s }
}

fn random_time_text(rng: &mut Rng) -> String {
    let mut s = if rng.below(20) == 0 {
        format!("{}:{}:{}.f", rng.pick(HOURS), rng.pick(MINUTES), rng.pick(SECONDS))
    } else {
        clock(rng)
    };
    s.push_str(rng.pick(OFFSETS));
    s.push_str(rng.pick(TRAILING));
    if rng.below(4) == 0 { mutate(rng, s) } else { s }
}

#[derive(Default)]
struct Report {
    cases: usize,
    accepted: usize,
    mismatches: Vec<String>,
}

impl Report {
    fn check<T: PartialEq + std::fmt::Debug>(
        &mut self,
        input: &str,
        ours: Option<T>,
        reference: Option<Option<T>>,
    ) {
        self.cases += 1;
        let ok = match (&ours, &reference) {
            (None, None) => true,
            (None, Some(None)) => true, // chrono accepted a value outside jiff's range
            (Some(a), Some(Some(b))) => {
                self.accepted += 1;
                a == b
            }
            _ => false,
        };
        if !ok {
            self.mismatches.push(format!(
                "{input:?}: ours={ours:?} chrono/sqlx={reference:?}"
            ));
        }
    }

    fn finish(self, name: &str) -> usize {
        println!(
            "[{name}] cases={} both-accept={} mismatches={}",
            self.cases,
            self.accepted,
            self.mismatches.len()
        );
        for m in self.mismatches.iter().take(25) {
            println!("  [{name}] {m}");
        }
        self.mismatches.len()
    }
}

#[test]
fn review_parsers_match_sqlx_chrono_glue() {
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let mut sqlite_local = Report::default();
    let mut sqlite_instant = Report::default();
    let mut date = Report::default();
    let mut time = Report::default();
    let mut pg_tz = Report::default();
    let mut pg_naive = Report::default();
    let mut mysql_dt = Report::default();
    let mut pg_time = Report::default();

    for _ in 0..300_000 {
        let s = random_date_time_text(&mut rng);
        let reference = sqlx_sqlite_datetime(&s);
        let ours = sqlite_date_time(&s);
        sqlite_local.check(
            &s,
            ours.as_ref().map(|m| m.local),
            reference.map(|d| civil_dt(&d.naive_local())),
        );
        // Timestamp decode: an out-of-jiff-range instant is a decode error.
        sqlite_instant.check(
            &s,
            ours.as_ref().and_then(|m| m.instant()),
            reference.map(|d| instant_of(&d.naive_utc())),
        );

        // `%F` (SQLite) and `%Y-%m-%d` (PostgreSQL, MySQL) are the same items.
        let ours_date = parse(&s, DATE_ONLY).and_then(|p| p.date());
        date.check(
            &s,
            ours_date,
            NaiveDate::parse_from_str(&s, "%F").ok().map(|d| civil_date(&d)),
        );
        assert_eq!(
            NaiveDate::parse_from_str(&s, "%F").ok(),
            NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
        );

        #[cfg(any(feature = "drivers-all", feature = "strict-postgres", feature = "pgvector"))]
        {
            use super::postgres::{date_time_from_text, instant_from_text};
            pg_tz.check(
                &s,
                instant_from_text(&s),
                sqlx_pg_timestamptz(&s).map(|d| instant_of(&d.naive_utc())),
            );
            pg_naive.check(
                &s,
                date_time_from_text(&s),
                sqlx_pg_timestamp(&s).map(|d| civil_dt(&d)),
            );
        }

        // MySQL text DATETIME.
        mysql_dt.check(
            &s,
            parse(&s, DATE_TIME).and_then(|p| p.date_time()),
            NaiveDateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f")
                .ok()
                .map(|d| civil_dt(&d)),
        );

        let t = random_time_text(&mut rng);
        time.check(
            &t,
            sqlite_time(&t),
            sqlx_sqlite_time(&t).map(|v| Some(civil_time(&v))),
        );
        // PostgreSQL and MySQL text TIME.
        pg_time.check(
            &t,
            parse(&t, TIME).and_then(|p| p.time()),
            NaiveTime::parse_from_str(&t, "%H:%M:%S%.f")
                .ok()
                .map(|v| Some(civil_time(&v))),
        );
    }

    let mismatches = sqlite_local.finish("sqlite datetime local")
        + sqlite_instant.finish("sqlite datetime instant")
        + date.finish("date %F")
        + time.finish("sqlite time")
        + pg_tz.finish("pg timestamptz text")
        + pg_naive.finish("pg timestamp text")
        + mysql_dt.finish("mysql/pg datetime text %Y-%m-%d %H:%M:%S%.f")
        + pg_time.finish("pg/mysql time text");
    assert_eq!(mismatches, 0);
}

#[test]
fn review_text_forms_match_chrono() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let min = jiff::Timestamp::MIN.as_second();
    let max = jiff::Timestamp::MAX.as_second();
    let mut mismatches = Vec::new();
    let mut checked = 0;
    for i in 0..300_000 {
        let secs = match i % 5 {
            0 => rng.range(min, max),
            1 => rng.range(-2_000_000_000, 2_000_000_000) + 946_684_800,
            2 => rng.range(-62_167_219_200 - 100_000, -62_167_219_200 + 100_000),
            3 => rng.range(min, min + 10_000_000),
            _ => rng.range(max - 10_000_000, max),
        };
        let nanos: u32 = match rng.below(5) {
            0 => 0,
            1 => (rng.below(1000) * 1_000_000) as u32,
            2 => (rng.below(1_000_000) * 1000) as u32,
            3 => rng.below(1_000_000_000) as u32,
            _ => rng.pick(&[1, 500, 999_999_999, 999_999_500, 1_000, 100_000_000, 10]),
        };
        let total = i128::from(secs) * 1_000_000_000 + i128::from(nanos);
        let Ok(ts) = jiff::Timestamp::from_nanosecond(total) else {
            continue;
        };
        checked += 1;
        let chrono = CDateTime::<Utc>::from_timestamp(secs, nanos).unwrap();
        let naive = chrono.naive_utc();
        let wall = text::utc(ts);
        let pairs = [
            (
                text::timestamp(Timestamp::from(ts)),
                chrono.to_rfc3339_opts(SecondsFormat::AutoSi, false),
            ),
            (text::date_time(DateTime::from(wall)), naive.format("%F %T%.f").to_string()),
            (text::date(Date::from(wall.date())), naive.format("%F").to_string()),
            (text::time(Time::from(wall.time())), naive.format("%T%.f").to_string()),
        ];
        for (ours, theirs) in pairs {
            if ours != theirs {
                mismatches.push(format!("{ts}: ours={ours:?} chrono={theirs:?}"));
            }
        }
        #[cfg(feature = "redis")]
        {
            let pairs = [
                (
                    text::timestamp_nanos(Timestamp::from(ts)),
                    chrono.to_rfc3339_opts(SecondsFormat::Nanos, true),
                ),
                (
                    text::date_time_nanos(DateTime::from(wall)),
                    naive.format("%F %T%.9f").to_string(),
                ),
                (text::time_nanos(Time::from(wall.time())), naive.format("%T%.9f").to_string()),
            ];
            for (ours, theirs) in pairs {
                if ours != theirs {
                    mismatches.push(format!("cache {ts}: ours={ours:?} chrono={theirs:?}"));
                }
            }
        }
    }
    println!("[text forms] checked={checked} mismatches={}", mismatches.len());
    for m in mismatches.iter().take(20) {
        println!("  {m}");
    }
    assert!(mismatches.is_empty());
}

/// Prints what changes for an application that swaps a chrono field for the
/// PR's type: serde output, accepted serde input, Display and Default.
#[test]
fn review_serde_display_default_differences() {
    let chrono_at = Utc.with_ymd_and_hms(2026, 10, 15, 9, 30, 0).unwrap()
        + chrono::TimeDelta::milliseconds(500);
    let ours: Timestamp = "2026-10-15T09:30:00.5Z".parse().unwrap();
    println!(
        "[serde out] DateTime<Utc>={} Timestamp={}",
        serde_json::to_string(&chrono_at).unwrap(),
        serde_json::to_string(&ours).unwrap()
    );
    let chrono_naive = chrono_at.naive_utc();
    let ours_naive = DateTime::from(text::utc(*ours));
    println!(
        "[serde out] NaiveDateTime={} DateTime={}",
        serde_json::to_string(&chrono_naive).unwrap(),
        serde_json::to_string(&ours_naive).unwrap()
    );
    println!(
        "[serde out] NaiveTime={} Time={}",
        serde_json::to_string(&chrono_naive.time()).unwrap(),
        serde_json::to_string(&Time::from(ours_naive.time())).unwrap()
    );
    println!(
        "[display] DateTime<Utc>={chrono_at} Timestamp={ours} | NaiveDateTime={chrono_naive} DateTime={ours_naive}"
    );

    let instants = [
        "2026-10-15T09:30:00Z",
        "2026-10-15 09:30:00Z",
        "2026-10-15t09:30:00z",
        "2026-10-15T09:30:00+0000",
        "2026-10-15T09:30:00+02",
        "2026-10-15T09:30:00 UTC",
        "2026-10-15T09:30:00UTC",
        "2026-10-15T09:30:00 +00:00",
        "2026-10-15T9:30:00Z",
        "2026-10-15T09:30Z",
        "2016-12-31T23:59:60Z",
        "+2026-10-15T09:30:00Z",
        "2026-10-15T09:30:00.1234567891Z",
        "2026-10-15T09:30:00,5Z",
        "2026-10-15T09:30:00-00:00",
        "2026-10-15T09:30:00+23:59",
    ];
    for input in instants {
        let json = format!("{input:?}");
        let c = serde_json::from_str::<CDateTime<Utc>>(&json).map(|v| v.to_rfc3339());
        let o = serde_json::from_str::<Timestamp>(&json).map(|v| v.to_string());
        if c.is_ok() != o.is_ok() {
            println!("[serde in Timestamp] {input:?}: chrono={:?} ours={:?}", c.ok(), o.ok());
        }
    }
    let naives = [
        "2026-10-15T09:30:00",
        "2026-10-15 09:30:00",
        "2026-10-15T09:30",
        "2026-10-15T09:30:60",
        " 2026-10-15T09:30:00",
        "2026-10-15T9:30:00",
        "+2026-10-15T09:30:00",
        "2026-10-15T09:30:00Z",
    ];
    for input in naives {
        let json = format!("{input:?}");
        let c = serde_json::from_str::<NaiveDateTime>(&json).map(|v| v.to_string());
        let o = serde_json::from_str::<DateTime>(&json).map(|v| v.to_string());
        if c.is_ok() != o.is_ok() {
            println!("[serde in DateTime] {input:?}: chrono={:?} ours={:?}", c.ok(), o.ok());
        }
    }
    for input in ["09:30", "9:30:00", "09:30:60", "09:30:00.5", "09:30:00Z", "24:00:00"] {
        let json = format!("{input:?}");
        let c = serde_json::from_str::<NaiveTime>(&json).map(|v| v.to_string());
        let o = serde_json::from_str::<Time>(&json).map(|v| v.to_string());
        if c.is_ok() != o.is_ok() {
            println!("[serde in Time] {input:?}: chrono={:?} ours={:?}", c.ok(), o.ok());
        }
    }
    for input in ["2026-10-15", "+2026-10-15", "2026-1-5", "2026-10-15T00:00:00"] {
        let json = format!("{input:?}");
        let c = serde_json::from_str::<NaiveDate>(&json).map(|v| v.to_string());
        let o = serde_json::from_str::<Date>(&json).map(|v| v.to_string());
        if c.is_ok() != o.is_ok() {
            println!("[serde in Date] {input:?}: chrono={:?} ours={:?}", c.ok(), o.ok());
        }
    }

    println!(
        "[default] NaiveDate={} Date={} | NaiveDateTime={} DateTime={} | DateTime<Utc>={} Timestamp={}",
        NaiveDate::default(),
        Date::default(),
        NaiveDateTime::default(),
        DateTime::default(),
        CDateTime::<Utc>::default(),
        Timestamp::default()
    );
    println!(
        "[default SQLite text] chrono date={:?} ours={:?}; chrono date-time={:?} ours={:?}",
        NaiveDate::default().format("%F").to_string(),
        text::date(Date::default()),
        NaiveDateTime::default().format("%F %T%.f").to_string(),
        text::date_time(DateTime::default())
    );
}
