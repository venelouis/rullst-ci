//! Scratch cross-check (not for merge): the CLI rule's POSIX form against
//! `posix_equivalent()` over the committed golden-vector corpus.
#![allow(clippy::all, dead_code)]

/// The longest expression `RecurringDefinition::new` accepts.
const MAX_EXPRESSION_BYTES: usize = 128;


/// What `expression`, as the `cron` crate read it, needs under POSIX, or
/// `None` when it means the same: no weekday number, not both day fields
/// restricted, and accepted by Core's evaluator.
fn advice(expression: &str) -> Option<String> {
    let fields: Vec<&str> = expression.split_whitespace().collect();
    if fields.iter().any(|field| shifted(field)) {
        return Some(format!(
            "`RecurringDefinition::new` now refuses `{expression}`: a field starts or ends with `,`, `-` or `/`, or runs a number into a name (`1jan`), so the `cron` crate joined or split fields and read the others shifted. A schedule already stored with it keeps running, but this call fails: stop creating it here, or cancel it and create a POSIX expression under a new name"
        ));
    }
    let [minute, hour, day, month, weekday] = fields[..] else {
        // The `cron` crate refused it too, at run time.
        return None;
    };
    let both_days = !day.starts_with(['*', '?']) && !weekday.starts_with(['*', '?']);
    let accepted = |candidate: &str| {
        rullst_core::Scheduler::new()
            .task(candidate, || async {})
            .is_ok()
    };
    let posix = weekdays(weekday).map(|weekday| format!("{minute} {hour} {day} {month} {weekday}"));
    let normalized = fields.join(" ");
    let mut notes = Vec::new();
    // AND before, OR now: no POSIX expression has the old schedule, as
    // messaging's `posix_equivalent()` also answers.
    if both_days {
        notes.push(format!(
            "it restricts both the day of the month (`{day}`) and the weekday (`{weekday}`): a schedule already stored keeps running on days that match both; one created after the upgrade runs on days that match either, so restrict one field and check the other when the message is handled"
        ));
    }
    match posix {
        None => notes.push(format!(
            "`{expression}` has a weekday number outside 1 to 7, which neither syntax accepts; write the POSIX expression by hand"
        )),
        Some(posix) if !accepted(&posix) && posix == normalized => notes.push(format!(
            "`RecurringDefinition::new` now refuses `{expression}`. A schedule already stored with it keeps running, but this call fails: stop creating it here, or cancel it and create a POSIX expression under a new name"
        )),
        Some(posix) if !accepted(&posix) => notes.push(format!(
            "no mechanical POSIX form: `{posix}`, its weekday numbers moved down by one, is refused; write a schedule not yet stored by hand"
        )),
        Some(posix) if posix == normalized || both_days => {}
        Some(posix) if posix.len() > MAX_EXPRESSION_BYTES => notes.push(format!(
            "no mechanical POSIX form: `{posix}`, its weekday numbers moved down by one, is longer than the 128 bytes `RecurringDefinition::new` accepts; write a schedule not yet stored by hand"
        )),
        Some(posix) => notes.push(format!(
            "POSIX form for a schedule not yet stored: `{posix}` (weekday numbers move down by one: Sunday is 0 or 7, Monday 1, Saturday 6)"
        )),
    }
    (!notes.is_empty()).then(|| notes.join("; "))
}

/// Whether the `cron` crate read `field` across a field boundary: it joined
/// a field that starts or ends with `,`, `-` or `/` with its neighbour
/// (`1, 2`, `/5`), and split one where a number and a name touch (`1jan`).
fn shifted(field: &str) -> bool {
    let touching = field.as_bytes().windows(2).any(|pair| {
        (pair[0].is_ascii_digit() && pair[1].is_ascii_alphabetic())
            || (pair[0].is_ascii_alphabetic() && pair[1].is_ascii_digit())
    });
    touching || field.starts_with([',', '-', '/']) || field.ends_with([',', '-', '/'])
}

/// The weekday field with each number moved from the `cron` crate's 1
/// (Sunday) to 7 (Saturday) to POSIX's 0 to 6; names, `*` and `?` stay. A
/// single day with a step (`2/3`, from that day to Saturday) gets the range
/// POSIX needs (`1-6/3`). `None` for a number outside 1 to 7.
fn weekdays(field: &str) -> Option<String> {
    let elements = field
        .split(',')
        .map(|element| {
            let (range, step) = match element.split_once('/') {
                Some((range, step)) => (range, Some(step)),
                None => (element, None),
            };
            let range = match range.split_once('-') {
                Some((start, end)) => format!("{}-{}", day(start)?, day(end)?),
                None if step.is_some() && is_number(range) => format!("{}-6", day(range)?),
                None => day(range)?,
            };
            Some(match step {
                Some(step) => format!("{range}/{step}"),
                None => range,
            })
        })
        .collect::<Option<Vec<_>>>()?;
    Some(elements.join(","))
}

/// One weekday: a number 1 to 7 moved down by one, anything else as it is.
fn day(text: &str) -> Option<String> {
    if !is_number(text) {
        return Some(text.to_string());
    }
    match text.parse::<u8>() {
        Ok(number @ 1..=7) => Some((number - 1).to_string()),
        _ => None,
    }
}

fn is_number(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}


const VECTORS: &str = include_str!("cron_expression_vectors.txt");

fn occurrences(expression: &str, start: i64) -> Result<Vec<Option<i64>>, String> {
    let mut after = start;
    let mut found = Vec::new();
    for _ in 0..6 {
        let next = rullst_core::scheduler::__test_hooks::cron_next_after(expression, after)?;
        found.push(next);
        match next { Some(at) => after = at, None => break }
    }
    Ok(found)
}

const STARTS: [i64; 6] = [0, 1_763_374_440_000, 1_709_164_799_999, 1_740_787_199_999, 1_767_225_599_000, 4_102_444_799_999];

fn mine(expression: &str) -> Option<String> {
    match advice(expression) {
        None => Some(expression.split_whitespace().collect::<Vec<_>>().join(" ")),
        Some(note) => note
            .split("POSIX form for a schedule not yet stored: `")
            .nth(1)
            .and_then(|rest| rest.split('`').next())
            .map(str::to_string),
    }
}

/// The recorded legacy occurrences: (expression, [(start, Some(results) | None for error)]).
fn corpus() -> Vec<(String, Vec<(i64, Option<Vec<Option<i64>>>)>)> {
    let mut out: Vec<(String, Vec<(i64, Option<Vec<Option<i64>>>)>)> = Vec::new();
    let mut current: Option<(String, Vec<(i64, Option<Vec<Option<i64>>>)>)> = None;
    for line in VECTORS.lines() {
        if line.starts_with('#') || line.is_empty() { continue; }
        if let Some(rest) = line.strip_prefix('\t') {
            let Some((start, results)) = rest.split_once('\t') else { continue };
            let start: i64 = start.parse().unwrap();
            let parsed = if results.trim() == "error" { None } else {
                let mut list = Vec::new();
                for token in results.split_whitespace() {
                    if token == "end" { list.push(None); break; }
                    if token == "error" { break; }
                    list.push(Some(token.parse::<i64>().unwrap()));
                }
                Some(list)
            };
            if let Some((_, starts)) = current.as_mut() { starts.push((start, parsed)); }
            continue;
        }
        if let Some(done) = current.take() { out.push(done); }
        let (expression, verdict) = line.rsplit_once('\t').unwrap();
        current = (verdict == "ok").then(|| (expression.to_string(), Vec::new()));
    }
    if let Some(done) = current.take() { out.push(done); }
    out
}

/// Whether `posix` (Core's evaluator) reproduces every recorded start.
fn matches_legacy(posix: &str, starts: &[(i64, Option<Vec<Option<i64>>>)]) -> Result<(), String> {
    for (start, recorded) in starts {
        let Some(recorded) = recorded else { continue };
        if !(-377_705_023_201_000..=253_402_207_200_000).contains(start) { continue; }
        let mut after = *start;
        for expected in recorded {
            let got = rullst_core::scheduler::__test_hooks::cron_next_after(posix, after)?;
            if got != *expected { return Err(format!("start {start}: expected {expected:?} got {got:?}")); }
            match got { Some(at) => after = at, None => break }
        }
    }
    Ok(())
}

#[test]
fn crosscheck_cli_note_against_posix_equivalent() {
    let mut agree = 0;
    let mut both_none = 0;
    let mut disagree = Vec::new();
    let mut mine_extra = 0;
    let mut mine_missing = 0;
    let mut mine_wrong = Vec::new();
    let mut theirs_wrong = Vec::new();
    let mut mine_checked = 0;
    let corpus = corpus();
    for (expression, starts) in &corpus {
        let expression = expression.as_str();
        let theirs = super::cron_expression::Schedule::parse_five_fields(expression).and_then(|s| s.posix_equivalent());
        let mine = mine(expression);
        if let Some(m) = &mine {
            mine_checked += 1;
            if let Err(why) = matches_legacy(m, starts) { mine_wrong.push(format!("{expression:?}: mine {m:?}: {why}; note {:?}", advice(expression))); }
        }
        if let Some(t) = &theirs {
            if let Err(why) = matches_legacy(t, starts) { theirs_wrong.push(format!("{expression:?}: theirs {t:?}: {why}")); }
        }
        match (&theirs, &mine) {
            (None, None) => both_none += 1,
            (Some(t), Some(m)) => {
                let same = STARTS.iter().all(|start| occurrences(t, *start) == occurrences(m, *start));
                if same { agree += 1 } else { disagree.push(format!("{expression:?}: theirs {t:?} mine {m:?}")) }
            }
            (None, Some(_)) => mine_extra += 1,
            (Some(_), None) => mine_missing += 1,
        }
    }
    let show = |items: &Vec<String>| items.iter().take(40).cloned().collect::<Vec<_>>().join("\n");
    panic!(
        "CROSSCHECK total={} agree={agree} both_none={both_none} disagree={} mine_extra={mine_extra} mine_missing={mine_missing} mine_checked_against_legacy={mine_checked} mine_wrong={} theirs_wrong={}\n--- disagree\n{}\n--- mine_wrong\n{}\n--- theirs_wrong\n{}\nEND",
        corpus.len(), disagree.len(), mine_wrong.len(), theirs_wrong.len(), show(&disagree), show(&mine_wrong), show(&theirs_wrong)
    );
}
