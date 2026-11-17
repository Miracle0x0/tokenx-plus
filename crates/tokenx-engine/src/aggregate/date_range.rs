//! Typed date filtering for canonical accumulation.

use jiff::civil::Date;
use serde::{Deserialize, Deserializer, Serialize};

/// Date filter evaluated from each finalized client-attributed usage record.
///
/// Bounds are inclusive. Construction validates the range once so acquisition,
/// cache identity, and aggregation can share one typed value without reparsing
/// or relying on lexicographic string ordering.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DateRange {
    since: Option<Date>,
    until: Option<Date>,
    year: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DateRangeError {
    #[error("date range start ({since}) must not be later than end ({until})")]
    ReversedBounds { since: Date, until: Date },
    #[error("invalid date range year ({year})")]
    InvalidYear { year: i32 },
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SerializedDateRange {
    since: Option<Date>,
    until: Option<Date>,
    year: Option<i32>,
}

impl<'de> Deserialize<'de> for DateRange {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let serialized = SerializedDateRange::deserialize(deserializer)?;
        Self::from_parts(serialized.since, serialized.until, serialized.year)
            .map_err(serde::de::Error::custom)
    }
}

impl DateRange {
    /// An inactive filter — keeps every message.
    pub fn none() -> Self {
        Self::default()
    }

    /// An inclusive date interval. Either bound may be omitted.
    pub fn bounded(since: Option<Date>, until: Option<Date>) -> Result<Self, DateRangeError> {
        Self::from_parts(since, until, None)
    }

    /// Every local date in one calendar year.
    pub fn for_year(year: i32) -> Result<Self, DateRangeError> {
        Self::from_parts(None, None, Some(year))
    }

    pub fn since(&self) -> Option<Date> {
        self.since
    }

    pub fn until(&self) -> Option<Date> {
        self.until
    }

    pub fn year(&self) -> Option<i32> {
        self.year
    }

    fn from_parts(
        since: Option<Date>,
        until: Option<Date>,
        year: Option<i32>,
    ) -> Result<Self, DateRangeError> {
        if let (Some(since), Some(until)) = (since, until) {
            if since > until {
                return Err(DateRangeError::ReversedBounds { since, until });
            }
        }
        if let Some(year) = year {
            if i16::try_from(year)
                .ok()
                .and_then(|year| Date::new(year, 1, 1).ok())
                .is_none()
            {
                return Err(DateRangeError::InvalidYear { year });
            }
        }
        Ok(Self { since, until, year })
    }

    pub(crate) fn is_unfiltered(&self) -> bool {
        self.year.is_none() && self.since.is_none() && self.until.is_none()
    }

    /// Whether one local calendar date passes this filter.
    pub fn contains(&self, date: Date) -> bool {
        if self.is_unfiltered() {
            return true;
        }
        let year_ok = self.year.is_none_or(|year| i32::from(date.year()) == year);
        let since_ok = self.since.is_none_or(|since| date >= since);
        let until_ok = self.until.is_none_or(|until| date <= until);
        year_ok && since_ok && until_ok
    }
}

#[cfg(test)]
mod date_range_tests {
    use super::*;

    fn date(year: i16, month: i8, day: i8) -> Date {
        Date::new(year, month, day).unwrap()
    }

    #[test]
    fn bounded_range_compares_typed_dates_inclusively() {
        let range = DateRange::bounded(Some(date(2026, 1, 31)), Some(date(2026, 2, 2))).unwrap();

        assert!(range.contains(date(2026, 1, 31)));
        assert!(range.contains(date(2026, 2, 1)));
        assert!(range.contains(date(2026, 2, 2)));
        assert!(!range.contains(date(2026, 1, 30)));
        assert!(!range.contains(date(2026, 2, 3)));
    }

    #[test]
    fn year_range_compares_calendar_year() {
        let range = DateRange::for_year(2026).unwrap();

        assert!(range.contains(date(2026, 1, 1)));
        assert!(range.contains(date(2026, 12, 31)));
        assert!(!range.contains(date(2025, 12, 31)));
    }

    #[test]
    fn reversed_bounds_are_rejected() {
        assert_eq!(
            DateRange::bounded(Some(date(2026, 2, 2)), Some(date(2026, 2, 1))),
            Err(DateRangeError::ReversedBounds {
                since: date(2026, 2, 2),
                until: date(2026, 2, 1),
            })
        );
    }

    #[test]
    fn deserialization_revalidates_bounds() {
        let error = serde_json::from_str::<DateRange>(
            r#"{"since":"2026-02-02","until":"2026-02-01","year":null}"#,
        )
        .unwrap_err();

        assert!(error.to_string().contains("must not be later"));
    }
}
