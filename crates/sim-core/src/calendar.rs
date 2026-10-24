//! Working calendars and the simulation clock (spec §5.6).
//!
//! Simulation time `t` is seconds since the simulation start, which is a real date and time
//! (`simulation.start`, default Monday 2024-01-01 00:00 UTC). Calendars use that date to know
//! the weekday. Only resources have calendars; arrivals, timers and message delays run on the
//! wall clock.

use serde::{Deserialize, Serialize};

const DAY: f64 = 86_400.0;

/// Working hours: one or more periods, each with its own days and hours. In a scenario a
/// single period is written as `days` + `hours`, several as `periods = [{ days, hours }, ...]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawCalendar", into = "RawCalendar")]
pub struct CalendarSpec {
    pub periods: Vec<PeriodSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PeriodSpec {
    /// Working days: "mon" .. "sun".
    pub days: Vec<String>,
    /// One or more comma-separated intervals, e.g. "09:00-12:00,13:00-17:00".
    pub hours: String,
}

/// How a calendar is written in a scenario.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCalendar {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    days: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hours: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    periods: Vec<PeriodSpec>,
}

impl TryFrom<RawCalendar> for CalendarSpec {
    type Error = String;

    fn try_from(r: RawCalendar) -> Result<Self, String> {
        let mut periods = match (r.days, r.hours) {
            (Some(days), Some(hours)) => vec![PeriodSpec { days, hours }],
            (None, None) => vec![],
            _ => return Err("a calendar needs both `days` and `hours`".into()),
        };
        periods.extend(r.periods);
        Ok(Self { periods })
    }
}

impl From<CalendarSpec> for RawCalendar {
    fn from(c: CalendarSpec) -> Self {
        match <[PeriodSpec; 1]>::try_from(c.periods) {
            Ok([p]) => Self {
                days: Some(p.days),
                hours: Some(p.hours),
                periods: vec![],
            },
            Err(periods) => Self {
                days: None,
                hours: None,
                periods,
            },
        }
    }
}

impl CalendarSpec {
    /// The same hours on each of `days`.
    pub fn uniform(days: &[&str], hours: &str) -> Self {
        Self {
            periods: vec![PeriodSpec {
                days: days.iter().map(|d| d.to_string()).collect(),
                hours: hours.into(),
            }],
        }
    }
}

/// Days from 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// Maps simulation seconds to real dates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimClock {
    /// Unix timestamp (UTC seconds) of simulation time 0.
    pub epoch: i64,
}

impl Default for SimClock {
    fn default() -> Self {
        // Monday 2024-01-01 00:00 UTC.
        Self {
            epoch: days_from_civil(2024, 1, 1) * 86_400,
        }
    }
}

impl SimClock {
    /// Parse "YYYY-MM-DD", "YYYY-MM-DDTHH:MM" or "YYYY-MM-DD HH:MM[:SS]" (UTC).
    pub fn parse(text: &str) -> Result<Self, String> {
        let bad = || format!("start '{text}': expected YYYY-MM-DD or YYYY-MM-DDTHH:MM[:SS]");
        let t = text.trim();
        let (date, time) = match t.split_once(['T', ' ']) {
            Some((d, tm)) => (d, tm),
            None => (t, "00:00"),
        };
        let mut dp = date.split('-').map(|x| x.parse::<i64>());
        let (Some(Ok(y)), Some(Ok(m)), Some(Ok(d)), None) =
            (dp.next(), dp.next(), dp.next(), dp.next())
        else {
            return Err(bad());
        };
        let mut tp = time.split(':').map(|x| x.parse::<i64>());
        let (Some(Ok(h)), Some(Ok(mi))) = (tp.next(), tp.next()) else {
            return Err(bad());
        };
        let s = match tp.next() {
            None => 0,
            Some(Ok(s)) => s,
            Some(Err(_)) => return Err(bad()),
        };
        if !(1..=12).contains(&m)
            || !(1..=31).contains(&d)
            || !(0..24).contains(&h)
            || !(0..60).contains(&mi)
            || !(0..60).contains(&s)
        {
            return Err(bad());
        }
        Ok(Self {
            epoch: days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60 + s,
        })
    }

    /// Seconds from the Monday 00:00 before the start to the start.
    pub fn week_offset(&self) -> f64 {
        let days = self.epoch.div_euclid(86_400);
        let weekday = (days + 3).rem_euclid(7); // 1970-01-01 was a Thursday; Monday = 0
        (weekday * 86_400 + self.epoch.rem_euclid(86_400)) as f64
    }

    /// ISO 8601 UTC timestamp of simulation time `t`.
    pub fn format(&self, t: f64) -> String {
        let secs = self.epoch + t.round() as i64;
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let sod = secs.rem_euclid(86_400);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
            sod / 3600,
            sod % 3600 / 60,
            sod % 60
        )
    }
}

pub const WEEKDAYS: [&str; 7] = ["mon", "tue", "wed", "thu", "fri", "sat", "sun"];

/// A resource calendar bound to a simulation start.
#[derive(Debug, Clone, PartialEq)]
pub struct Calendar {
    /// Per weekday (Monday first): sorted, non-overlapping (start, end) in seconds of the day.
    intervals: [Vec<(f64, f64)>; 7],
    /// Seconds from Monday 00:00 to simulation time 0.
    offset: f64,
}

fn parse_hhmm(s: &str) -> Option<f64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (m < 60 && (h < 24 || (h == 24 && m == 0))).then(|| f64::from(h * 3600 + m * 60))
}

impl Calendar {
    pub fn new(name: &str, spec: &CalendarSpec, clock: SimClock) -> Result<Self, String> {
        let mut intervals: [Vec<(f64, f64)>; 7] = Default::default();
        for p in &spec.periods {
            let mut days = Vec::new();
            for d in &p.days {
                let i = WEEKDAYS
                    .iter()
                    .position(|x| x.eq_ignore_ascii_case(d))
                    .ok_or_else(|| {
                        format!("calendar '{name}': unknown day '{d}' (use mon..sun)")
                    })?;
                days.push(i);
            }
            if days.is_empty() && spec.periods.len() > 1 {
                return Err(format!(
                    "calendar '{name}': hours '{}' are on no day",
                    p.hours
                ));
            }
            for part in p.hours.split(',') {
                let bad =
                    || format!("calendar '{name}': hours '{part}' must look like 09:00-17:00");
                let (a, b) = part.split_once('-').ok_or_else(bad)?;
                let (a, b) = (
                    parse_hhmm(a).ok_or_else(bad)?,
                    parse_hhmm(b).ok_or_else(bad)?,
                );
                if a >= b {
                    return Err(format!(
                        "calendar '{name}': interval '{part}' must end after it starts (no overnight intervals)"
                    ));
                }
                for &i in &days {
                    intervals[i].push((a, b));
                }
            }
        }
        for (day, iv) in WEEKDAYS.iter().zip(&mut intervals) {
            iv.sort_by(|x, y| x.0.total_cmp(&y.0));
            iv.dedup();
            if iv.windows(2).any(|w| w[0].1 > w[1].0) {
                return Err(format!("calendar '{name}': hours overlap on {day}"));
            }
        }
        if intervals.iter().all(Vec::is_empty) {
            return Err(format!("calendar '{name}': no working time"));
        }
        Ok(Self {
            intervals,
            offset: clock.week_offset(),
        })
    }

    /// Open spans (absolute seconds on the calendar's own timeline) of day number `d`.
    fn spans(&self, d: i64) -> impl Iterator<Item = (f64, f64)> + '_ {
        let base = d as f64 * DAY;
        self.intervals[d.rem_euclid(7) as usize]
            .iter()
            .map(move |(a, b)| (base + a, base + b))
    }

    /// Earliest time `>= t` at which the calendar is open.
    pub fn next_open(&self, t: f64) -> f64 {
        let u = t + self.offset;
        let first = (u / DAY).floor() as i64;
        for d in first..first + 8 {
            for (a, b) in self.spans(d) {
                if b > u {
                    return a.max(u) - self.offset;
                }
            }
        }
        t // unreachable: a calendar has working time in every week
    }

    /// Time at which `work` seconds of working time, started at `t`, are done. Work pauses
    /// outside opening hours and continues at the next opening.
    pub fn advance(&self, t: f64, work: f64) -> f64 {
        let mut u = self.next_open(t) + self.offset;
        let mut remaining = work;
        loop {
            let day = (u / DAY).floor() as i64;
            for d in day..day + 8 {
                for (a, b) in self.spans(d) {
                    if b <= u {
                        continue;
                    }
                    let from = a.max(u);
                    if remaining <= b - from {
                        return from + remaining - self.offset;
                    }
                    remaining -= b - from;
                    u = b;
                }
            }
            u += 7.0 * DAY; // (only reached after 8 days without time: cannot happen)
        }
    }

    /// Working time between `a` and `b` (0 if `b <= a`).
    pub fn open_between(&self, a: f64, b: f64) -> f64 {
        if b <= a {
            return 0.0;
        }
        let (ua, ub) = (a + self.offset, b + self.offset);
        let mut total = 0.0;
        for d in (ua / DAY).floor() as i64..=(ub / DAY).floor() as i64 {
            for (s, e) in self.spans(d) {
                total += (e.min(ub) - s.max(ua)).max(0.0);
            }
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn office(start: &str) -> Calendar {
        let spec = CalendarSpec::uniform(&WEEKDAYS[..5], "09:00-17:00");
        Calendar::new("office", &spec, SimClock::parse(start).unwrap()).unwrap()
    }

    const H: f64 = 3600.0;

    #[test]
    fn dates_round_trip_and_weekdays() {
        let c = SimClock::parse("2026-01-05T08:00").unwrap(); // a Monday
        assert_eq!(c.week_offset(), 8.0 * H);
        assert_eq!(c.format(0.0), "2026-01-05T08:00:00Z");
        assert_eq!(c.format(86_400.0 * 30.0 + 61.0), "2026-02-04T08:01:01Z");
        assert_eq!(
            SimClock::parse("2024-02-29 23:59:59").unwrap().format(1.0),
            "2024-03-01T00:00:00Z"
        );
        assert_eq!(SimClock::default().week_offset(), 0.0);
        assert!(SimClock::parse("yesterday").is_err());
    }

    #[test]
    fn work_waits_for_opening_and_pauses_overnight() {
        let c = office("2024-01-01T00:00"); // Monday 00:00
        assert_eq!(c.next_open(0.0), 9.0 * H);
        assert_eq!(c.next_open(10.0 * H), 10.0 * H);
        assert_eq!(
            c.next_open(17.0 * H),
            24.0 * H + 9.0 * H,
            "closing time itself is closed"
        );
        // 3 hours starting Monday 15:00: 2h until 17:00, 1h on Tuesday from 09:00.
        assert_eq!(c.advance(15.0 * H, 3.0 * H), 24.0 * H + 10.0 * H);
        // Ends exactly at closing time.
        assert_eq!(c.advance(15.0 * H, 2.0 * H), 17.0 * H);
        // Starting outside opening hours begins at the next opening.
        assert_eq!(c.advance(20.0 * H, 1.0 * H), 24.0 * H + 10.0 * H);
        // Friday 16:00 + 2h skips the weekend.
        let fri = 4.0 * 86_400.0 + 16.0 * H;
        assert_eq!(c.advance(fri, 2.0 * H), 7.0 * 86_400.0 + 10.0 * H);
        // Eight hours of work fill a day exactly; a 40h task takes a whole week.
        assert_eq!(c.advance(9.0 * H, 8.0 * H), 17.0 * H);
        assert_eq!(c.advance(9.0 * H, 40.0 * H), 4.0 * 86_400.0 + 17.0 * H);
    }

    #[test]
    fn open_time_between_two_instants() {
        let c = office("2024-01-01T00:00");
        assert_eq!(c.open_between(0.0, 86_400.0), 8.0 * H);
        assert_eq!(c.open_between(0.0, 7.0 * 86_400.0), 40.0 * H);
        assert_eq!(c.open_between(12.0 * H, 13.0 * H), H);
        assert_eq!(c.open_between(18.0 * H, 20.0 * H), 0.0);
        assert_eq!(c.open_between(5.0, 5.0), 0.0);
    }

    #[test]
    fn start_time_decides_the_weekday() {
        // Starting on a Saturday: first opening is Monday 09:00 = 2 days + 9h later.
        let c = office("2024-01-06T00:00");
        assert_eq!(c.next_open(0.0), 2.0 * 86_400.0 + 9.0 * H);
    }

    #[test]
    fn split_shifts_and_bad_input() {
        let spec = CalendarSpec::uniform(&["mon"], "09:00-12:00, 13:00-17:00");
        let c = Calendar::new("split", &spec, SimClock::default()).unwrap();
        assert_eq!(c.advance(11.0 * H, 2.0 * H), 14.0 * H);
        for (days, hours) in [
            (vec!["xyz"], "09:00-17:00"),
            (vec!["mon"], "17:00-09:00"),
            (vec!["mon"], "09:00-12:00,11:00-13:00"),
            (vec![], "09:00-17:00"),
            (vec!["mon"], "9-17"),
        ] {
            let spec = CalendarSpec::uniform(&days, hours);
            assert!(
                Calendar::new("bad", &spec, SimClock::default()).is_err(),
                "{hours}"
            );
        }
    }

    #[test]
    fn different_hours_on_different_days() {
        let toml = r#"periods = [
            { days = ["mon", "tue", "wed", "thu", "fri"], hours = "20:00-24:00" },
            { days = ["sat", "sun"], hours = "08:00-24:00" },
        ]"#;
        let spec: CalendarSpec = toml::from_str(toml).unwrap();
        let c = Calendar::new("shifts", &spec, SimClock::default()).unwrap(); // Monday 00:00
        assert_eq!(c.next_open(0.0), 20.0 * H);
        assert_eq!(
            c.open_between(0.0, 7.0 * DAY),
            5.0 * 4.0 * H + 2.0 * 16.0 * H
        );
        // Friday 23:00 + 2h: 1h until midnight, 1h from Saturday 08:00.
        assert_eq!(
            c.advance(4.0 * DAY + 23.0 * H, 2.0 * H),
            5.0 * DAY + 9.0 * H
        );
        // Days may appear in several periods as long as the hours do not overlap.
        let spec = CalendarSpec {
            periods: vec![
                PeriodSpec {
                    days: vec!["mon".into()],
                    hours: "08:00-12:00".into(),
                },
                PeriodSpec {
                    days: vec!["mon".into()],
                    hours: "11:00-13:00".into(),
                },
            ],
        };
        let e = Calendar::new("x", &spec, SimClock::default()).unwrap_err();
        assert!(e.contains("overlap on mon"), "{e}");
        let spec = CalendarSpec {
            periods: vec![
                PeriodSpec {
                    days: vec!["mon".into()],
                    hours: "08:00-12:00".into(),
                },
                PeriodSpec {
                    days: vec![],
                    hours: "13:00-17:00".into(),
                },
            ],
        };
        let e = Calendar::new("x", &spec, SimClock::default()).unwrap_err();
        assert!(e.contains("on no day"), "{e}");
    }

    #[test]
    fn one_period_is_written_as_days_and_hours() {
        let one = CalendarSpec::uniform(&["mon"], "09:00-17:00");
        let text = toml::to_string(&one).unwrap();
        assert_eq!(text, "days = [\"mon\"]\nhours = \"09:00-17:00\"\n");
        assert_eq!(toml::from_str::<CalendarSpec>(&text).unwrap(), one);
        let mut two = one.clone();
        two.periods.push(PeriodSpec {
            days: vec!["sat".into()],
            hours: "08:00-12:00".into(),
        });
        let text = toml::to_string(&two).unwrap();
        assert!(text.contains("periods"), "{text}");
        assert_eq!(toml::from_str::<CalendarSpec>(&text).unwrap(), two);
        assert!(toml::from_str::<CalendarSpec>("days = [\"mon\"]").is_err());
        assert!(
            toml::from_str::<CalendarSpec>("days = [\"mon\"]\nhours = \"9-5\"\nx = 1").is_err()
        );
    }
}
