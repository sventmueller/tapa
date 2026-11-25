//! Durations, token counts, money and times as the interface writes them.

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2_foundation::{
    NSDate, NSDateFormatter, NSDateFormatterStyle, NSISO8601DateFormatter, NSNumber, NSNumberFormatter,
    NSNumberFormatterStyle, NSRelativeDateTimeFormatter, NSRelativeDateTimeFormatterStyle,
    NSRelativeDateTimeFormatterUnitsStyle,
};
use splat_i18n::{t, tf};

use crate::common::util::ns;

/// A running time like `< 1 s`, `12 s`, `3 min 5 s` or `2 h 4 min`.
pub fn elapsed(duration: Duration) -> String {
    let seconds = duration.as_secs();
    match seconds {
        0 => t("< 1 s").to_string(),
        1..60 => tf("{} s", &[&seconds.to_string()]),
        60..3600 => tf(
            "{} min {} s",
            &[&(seconds / 60).to_string(), &(seconds % 60).to_string()],
        ),
        _ => tf(
            "{} h {} min",
            &[&(seconds / 3600).to_string(), &((seconds % 3600) / 60).to_string()],
        ),
    }
}

/// A token count like `950` or `13 k`.
pub fn tokens(count: u64) -> String {
    match count {
        0..1000 => count.to_string(),
        1000..1_000_000 => tf("{} k", &[&(count.div_ceil(1000)).to_string()]),
        _ => tf("{} M", &[&format!("{:.1}", count as f64 / 1_000_000.0)]),
    }
}

/// US dollars in the user's number format.
pub(crate) fn dollars(amount: f64) -> String {
    let formatter = NSNumberFormatter::new();
    formatter.setNumberStyle(NSNumberFormatterStyle::CurrencyStyle);
    formatter.setCurrencyCode(Some(&ns("USD")));
    formatter.setMaximumFractionDigits(if amount < 1.0 { 3 } else { 2 });
    formatter
        .stringFromNumber(&NSNumber::new_f64(amount))
        .map_or_else(|| format!("${amount:.2}"), |s| s.to_string())
}

/// A cost in the unit supplied by the agent, without currency conversion.
pub(crate) fn reported_cost(cost: &term_thread::ReportedCost) -> String {
    if cost.currency == "USD" {
        return dollars(cost.amount);
    }
    let formatter = NSNumberFormatter::new();
    formatter.setNumberStyle(NSNumberFormatterStyle::DecimalStyle);
    formatter.setMaximumFractionDigits(6);
    let amount = formatter
        .stringFromNumber(&NSNumber::new_f64(cost.amount))
        .map_or_else(|| cost.amount.to_string(), |s| s.to_string());
    format!("{amount} {}", cost.currency)
}

/// A share, `0.42` for 42 percent, in the user's number format.
#[cfg_attr(
    not(feature = "selftest"),
    expect(unreachable_pub, reason = "the self-test re-exports it")
)]
pub fn percent(share: f64) -> String {
    let formatter = NSNumberFormatter::new();
    formatter.setNumberStyle(NSNumberFormatterStyle::PercentStyle);
    formatter.setMaximumFractionDigits(0);
    formatter
        .stringFromNumber(&NSNumber::new_f64(share))
        .map_or_else(|| format!("{:.0} %", share * 100.0), |s| s.to_string())
}

fn date(at: SystemTime) -> Retained<NSDate> {
    let seconds = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64();
    NSDate::dateWithTimeIntervalSince1970(seconds)
}

/// The time of day, like `5:31 PM`, in the user's format; with the date when it is not today.
pub(crate) fn time_of_day(at: SystemTime) -> String {
    let formatter = NSDateFormatter::new();
    formatter.setTimeStyle(NSDateFormatterStyle::ShortStyle);
    let now = SystemTime::now();
    let apart = now
        .duration_since(at)
        .or_else(|_| at.duration_since(now))
        .unwrap_or_default();
    let today = apart < Duration::from_secs(12 * 3600);
    formatter.setDateStyle(if today {
        NSDateFormatterStyle::NoStyle
    } else {
        NSDateFormatterStyle::ShortStyle
    });
    formatter.stringFromDate(&date(at)).to_string()
}

/// An ISO 8601 time, such as `2026-11-09T22:00:00Z`, as `time_of_day` writes it; the text as it is when it is none.
pub(crate) fn iso_time(text: &str) -> String {
    let parsed = NSISO8601DateFormatter::new().dateFromString(&ns(text));
    match parsed {
        Some(date) => {
            let seconds = date.timeIntervalSince1970().max(0.0);
            time_of_day(UNIX_EPOCH + Duration::from_secs_f64(seconds))
        }
        None => text.to_string(),
    }
}

/// How long ago `at` was, like `5 min ago`, in the user's language.
pub fn age(at: SystemTime) -> String {
    let at = at.min(SystemTime::now());
    RELATIVE.with(|formatter| {
        formatter
            .localizedStringForDate_relativeToDate(&date(at), &NSDate::now())
            .to_string()
    })
}

thread_local! {
    // Formatters take long to make and lists ask for an age per row as they scroll.
    static RELATIVE: Retained<NSRelativeDateTimeFormatter> = {
        let formatter = NSRelativeDateTimeFormatter::new();
        // The abbreviated style mixes `vor 7 m` with `vor 2 Std.` in German; the short one abbreviates alike.
        formatter.setUnitsStyle(NSRelativeDateTimeFormatterUnitsStyle::Short);
        formatter.setDateTimeStyle(NSRelativeDateTimeFormatterStyle::Named);
        formatter
    };
}

/// `path` relative to `cwd` when it lies inside, else with the home folder as `~`.
pub(crate) fn short_path(path: &str, cwd: &Path) -> String {
    let p = Path::new(path);
    match p.strip_prefix(cwd) {
        Ok(rest) if !rest.as_os_str().is_empty() => rest.display().to_string(),
        _ => crate::common::panel::display_path(p),
    }
}

/// The first line of `text`, at most `limit` characters, with an ellipsis when cut.
pub(crate) fn first_line(text: &str, limit: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim();
    let mut out: String = line.chars().take(limit).collect();
    // Looking no further than the second line, as a streamed answer asks for this with each part that arrives.
    if line.chars().nth(limit).is_some() || text.trim().lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_counts_read_short() {
        assert_eq!(elapsed(Duration::from_millis(300)), "< 1 s");
        assert_eq!(elapsed(Duration::from_secs(12)), "12 s");
        assert_eq!(elapsed(Duration::from_secs(185)), "3 min 5 s");
        assert_eq!(elapsed(Duration::from_secs(7440)), "2 h 4 min");
        assert_eq!(tokens(950), "950");
        assert_eq!(tokens(12_960), "13 k");
    }

    #[test]
    fn a_first_line_is_cut_with_an_ellipsis() {
        assert_eq!(first_line("one\ntwo", 10), "one…");
        assert_eq!(first_line("abcdef", 3), "abc…");
        assert_eq!(first_line("abc", 3), "abc");
        assert_eq!(first_line("\n  one  \n\n", 10), "one");
        assert_eq!(first_line("one\n\ntwo\nthree", 10), "one…");
        assert_eq!(first_line("äöü", 2), "äö…");
        assert_eq!(first_line("äöü", 3), "äöü");
    }

    #[test]
    fn paths_inside_the_folder_are_shown_relative() {
        let cwd = Path::new("/work/project");
        assert_eq!(short_path("/work/project/src/a.rs", cwd), "src/a.rs");
        assert_eq!(short_path("/elsewhere/b.rs", cwd), "/elsewhere/b.rs");
    }
}
