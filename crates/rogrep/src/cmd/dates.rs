//! Resolve in-query date facets (`since:`/`after:`/`before:`/`until:`/
//! `when:`) plus the `--since` flag into one timestamp range for the
//! index's `[from, to)` ts filter.

use anyhow::{bail, Result};
use jiff::tz::TimeZone;

/// Parse a date facet value: `YYYY-MM-DD` (start of that local day) or
/// `Nd` (N days before now).
pub fn parse_date_ms(spec: &str, tz: &TimeZone) -> Result<i64> {
    let spec = spec.trim();
    if let Some(days) = spec.strip_suffix('d').and_then(|d| d.parse::<i64>().ok()) {
        return Ok(jiff::Timestamp::now().as_millisecond() - days * 86_400_000);
    }
    if let Ok(date) = spec.parse::<jiff::civil::Date>() {
        return Ok(date.to_zoned(tz.clone())?.timestamp().as_millisecond());
    }
    bail!("cannot parse date {spec:?}; use YYYY-MM-DD or Nd")
}

/// Start of the local day after the given date value; for `Nd` values the
/// range is open-ended, so this returns None.
fn next_day_ms(spec: &str, tz: &TimeZone) -> Result<Option<i64>> {
    let spec = spec.trim();
    if let Ok(date) = spec.parse::<jiff::civil::Date>() {
        let next = date.checked_add(jiff::Span::new().days(1))?;
        return Ok(Some(next.to_zoned(tz.clone())?.timestamp().as_millisecond()));
    }
    Ok(None)
}

/// Intersect all date facets with the `--since` flag. `since:`/`after:`
/// are inclusive lower bounds, `before:`/`until:` exclusive upper bounds,
/// `when:DATE` covers that one day (`when:Nd` behaves like `since:Nd`).
pub fn resolve_dates(
    dates: &[(String, String)],
    flag_since: Option<i64>,
    tz: &TimeZone,
) -> Result<Option<(i64, i64)>> {
    let mut lo = flag_since;
    let mut hi: Option<i64> = None;
    let tighten_lo = |v: i64, lo: &mut Option<i64>| *lo = Some(lo.map_or(v, |c: i64| c.max(v)));
    let tighten_hi = |v: i64, hi: &mut Option<i64>| *hi = Some(hi.map_or(v, |c: i64| c.min(v)));
    for (key, value) in dates {
        match key.as_str() {
            "since" | "after" => tighten_lo(parse_date_ms(value, tz)?, &mut lo),
            "before" | "until" => tighten_hi(parse_date_ms(value, tz)?, &mut hi),
            "when" => {
                tighten_lo(parse_date_ms(value, tz)?, &mut lo);
                if let Some(end) = next_day_ms(value, tz)? {
                    tighten_hi(end, &mut hi);
                }
            }
            _ => bail!("unknown date facet {key}:"),
        }
    }
    match (lo, hi) {
        (None, None) => Ok(None),
        (lo, hi) => {
            let range = (lo.unwrap_or(i64::MIN), hi.unwrap_or(i64::MAX));
            if range.0 >= range.1 {
                bail!("date facets produce an empty range");
            }
            Ok(Some(range))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> TimeZone {
        TimeZone::UTC
    }

    fn facets(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(resolve_dates(&[], None, &tz()).unwrap(), None);
    }

    #[test]
    fn since_and_before_bound_both_ends() {
        let r = resolve_dates(
            &facets(&[("since", "2026-07-01"), ("before", "2026-08-01")]),
            None,
            &tz(),
        )
        .unwrap()
        .unwrap();
        assert!(r.0 < r.1);
        assert_eq!(r.1 - r.0, 31 * 86_400_000);
    }

    #[test]
    fn when_covers_one_day() {
        let r = resolve_dates(&facets(&[("when", "2026-07-15")]), None, &tz())
            .unwrap()
            .unwrap();
        assert_eq!(r.1 - r.0, 86_400_000);
    }

    #[test]
    fn relative_days_parse() {
        let r = resolve_dates(&facets(&[("since", "7d")]), None, &tz()).unwrap().unwrap();
        assert!(r.0 > 0 && r.1 == i64::MAX);
    }

    #[test]
    fn flag_intersects_facets() {
        // Flag is later than the facet: the tighter (later) bound wins.
        let flag = jiff::Timestamp::now().as_millisecond() - 86_400_000;
        let r = resolve_dates(&facets(&[("since", "2020-01-01")]), Some(flag), &tz())
            .unwrap()
            .unwrap();
        assert_eq!(r.0, flag);
    }

    #[test]
    fn empty_range_errors() {
        let err = resolve_dates(
            &facets(&[("since", "2026-08-01"), ("before", "2026-07-01")]),
            None,
            &tz(),
        )
        .unwrap_err();
        assert!(err.to_string().contains("empty range"));
    }

    #[test]
    fn bad_value_errors() {
        assert!(resolve_dates(&facets(&[("since", "yesterday")]), None, &tz()).is_err());
    }
}
