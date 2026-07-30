use std::borrow::Cow;

use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike, Weekday};
use tokenx_engine::projection::PeriodKind;

pub(crate) fn month_name(month: u32, short: bool) -> Cow<'static, str> {
    match (month, short) {
        (1, false) => rust_i18n::t!("tui.date.month.jan"),
        (2, false) => rust_i18n::t!("tui.date.month.feb"),
        (3, false) => rust_i18n::t!("tui.date.month.mar"),
        (4, false) => rust_i18n::t!("tui.date.month.apr"),
        (5, false) => rust_i18n::t!("tui.date.month.may"),
        (6, false) => rust_i18n::t!("tui.date.month.jun"),
        (7, false) => rust_i18n::t!("tui.date.month.jul"),
        (8, false) => rust_i18n::t!("tui.date.month.aug"),
        (9, false) => rust_i18n::t!("tui.date.month.sep"),
        (10, false) => rust_i18n::t!("tui.date.month.oct"),
        (11, false) => rust_i18n::t!("tui.date.month.nov"),
        (12, false) => rust_i18n::t!("tui.date.month.dec"),
        (1, true) => rust_i18n::t!("tui.date.month_short.jan"),
        (2, true) => rust_i18n::t!("tui.date.month_short.feb"),
        (3, true) => rust_i18n::t!("tui.date.month_short.mar"),
        (4, true) => rust_i18n::t!("tui.date.month_short.apr"),
        (5, true) => rust_i18n::t!("tui.date.month_short.may"),
        (6, true) => rust_i18n::t!("tui.date.month_short.jun"),
        (7, true) => rust_i18n::t!("tui.date.month_short.jul"),
        (8, true) => rust_i18n::t!("tui.date.month_short.aug"),
        (9, true) => rust_i18n::t!("tui.date.month_short.sep"),
        (10, true) => rust_i18n::t!("tui.date.month_short.oct"),
        (11, true) => rust_i18n::t!("tui.date.month_short.nov"),
        (12, true) => rust_i18n::t!("tui.date.month_short.dec"),
        _ => Cow::Borrowed(""),
    }
}

pub(crate) fn weekday_name(weekday: Weekday) -> Cow<'static, str> {
    match weekday {
        Weekday::Mon => rust_i18n::t!("tui.date.weekday_short.mon"),
        Weekday::Tue => rust_i18n::t!("tui.date.weekday_short.tue"),
        Weekday::Wed => rust_i18n::t!("tui.date.weekday_short.wed"),
        Weekday::Thu => rust_i18n::t!("tui.date.weekday_short.thu"),
        Weekday::Fri => rust_i18n::t!("tui.date.weekday_short.fri"),
        Weekday::Sat => rust_i18n::t!("tui.date.weekday_short.sat"),
        Weekday::Sun => rust_i18n::t!("tui.date.weekday_short.sun"),
    }
}

pub(crate) fn format_month_day(date: NaiveDate) -> String {
    rust_i18n::t!(
        "tui.date.month_day",
        month = month_name(date.month(), true),
        day = date.day()
    )
    .into_owned()
}

pub(crate) fn format_numeric_month_day(date: NaiveDate) -> String {
    rust_i18n::t!(
        "tui.date.month_day_numeric",
        month = format!("{:02}", date.month()),
        day = format!("{:02}", date.day())
    )
    .into_owned()
}

pub(crate) fn format_month_year(date: NaiveDate) -> String {
    rust_i18n::t!(
        "tui.date.month_year",
        month = month_name(date.month(), false),
        year = date.year()
    )
    .into_owned()
}

pub(crate) fn format_month_separator(date: NaiveDate) -> String {
    format_month_separator_for_locale(date, &rust_i18n::locale())
}

pub(crate) fn format_month_separator_for_locale(date: NaiveDate, locale: &str) -> String {
    rust_i18n::t!(
        "tui.date.month_separator",
        locale = locale,
        month = format!("{:02}", date.month()),
        year = date.year()
    )
    .into_owned()
}

pub(crate) fn format_day_weekday(date: NaiveDate) -> String {
    rust_i18n::t!(
        "tui.date.day_weekday",
        day = format!("{:02}", date.day()),
        weekday = weekday_name(date.weekday())
    )
    .into_owned()
}

pub(crate) fn format_full_date(date: NaiveDate) -> String {
    rust_i18n::t!(
        "tui.date.full_date",
        weekday = weekday_name(date.weekday()),
        month = month_name(date.month(), true),
        day = format!("{:02}", date.day()),
        year = date.year()
    )
    .into_owned()
}

pub(crate) fn format_year_month_day(date: NaiveDate) -> String {
    rust_i18n::t!(
        "tui.date.year_month_day",
        year = date.year(),
        month = format!("{:02}", date.month()),
        day = format!("{:02}", date.day())
    )
    .into_owned()
}

pub(crate) fn format_timestamp(datetime: NaiveDateTime) -> String {
    format_timestamp_for_locale(datetime, &rust_i18n::locale())
}

fn format_timestamp_for_locale(datetime: NaiveDateTime, locale: &str) -> String {
    rust_i18n::t!(
        "tui.date.timestamp",
        locale = locale,
        month = format!("{:02}", datetime.month()),
        day = format!("{:02}", datetime.day()),
        hour = format!("{:02}", datetime.hour()),
        minute = format!("{:02}", datetime.minute())
    )
    .into_owned()
}
pub(crate) fn format_clock_time(datetime: NaiveDateTime) -> String {
    let hour = match datetime.hour() % 12 {
        0 => 12,
        hour => hour,
    };
    let period = if datetime.hour() < 12 {
        rust_i18n::t!("tui.date.time.am")
    } else {
        rust_i18n::t!("tui.date.time.pm")
    };
    rust_i18n::t!("tui.date.time.format", hour = hour, period = period).into_owned()
}

pub(crate) fn format_period_label(
    kind: PeriodKind,
    start: NaiveDate,
    end: NaiveDate,
    short: bool,
) -> String {
    match kind {
        PeriodKind::Monthly => month_name(start.month(), short).into_owned(),
        PeriodKind::Weekly => {
            format_weekly_period_label_for_locale(start, end, short, &rust_i18n::locale())
        }
    }
}

fn format_weekly_period_label_for_locale(
    start: NaiveDate,
    end: NaiveDate,
    _short: bool,
    locale: &str,
) -> String {
    let compact_month_day = |date: NaiveDate| format!("{:02}/{:02}", date.month(), date.day());

    rust_i18n::t!(
        "tui.date.week_range",
        locale = locale,
        week = format!("{:02}", start.iso_week().week()),
        start = compact_month_day(start),
        end = compact_month_day(end)
    )
    .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_text::width_u16;

    #[test]
    fn chinese_full_date_has_only_one_month_suffix() {
        let rendered = rust_i18n::t!(
            "tui.date.full_date",
            locale = "zh-CN",
            weekday = "周日",
            month = "1月",
            day = "02",
            year = 2026
        );
        assert_eq!(rendered, "2026年1月02日（周日）");
    }

    #[test]
    fn localized_clock_time_uses_translated_period() {
        let period = rust_i18n::t!("tui.date.time.am", locale = "zh-CN");
        let rendered = rust_i18n::t!(
            "tui.date.time.format",
            locale = "zh-CN",
            hour = 9,
            period = period
        );
        assert_eq!(rendered, "上午9点");
    }

    #[test]
    fn weekly_period_labels_share_one_compact_numeric_contract() {
        let start = NaiveDate::from_ymd_opt(2026, 7, 27).unwrap();
        let end = NaiveDate::from_ymd_opt(2026, 8, 2).unwrap();

        for short in [false, true] {
            assert_eq!(
                format_weekly_period_label_for_locale(start, end, short, "en"),
                "Week31  07/27–08/02"
            );
            assert_eq!(
                format_weekly_period_label_for_locale(start, end, short, "zh-CN"),
                "第31周  07/27–08/02"
            );
        }
    }

    #[test]
    fn timestamps_share_one_compact_numeric_contract() {
        let datetime = NaiveDate::from_ymd_opt(2026, 7, 29)
            .unwrap()
            .and_hms_opt(20, 35, 0)
            .unwrap();

        for locale in ["en", "zh-CN"] {
            let rendered = format_timestamp_for_locale(datetime, locale);
            assert_eq!(rendered, "07/29 20:35");
            assert_eq!(width_u16(&rendered), 11);
        }
    }

    #[test]
    fn chinese_month_separator_width_includes_both_suffixes() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 27).unwrap();
        let rendered = format_month_separator_for_locale(date, "zh-CN");

        assert_eq!(rendered, "2026年07月");
        assert_eq!(width_u16(&rendered), 10);
    }
}
